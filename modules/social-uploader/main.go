package main

import (
	"context"
	"crypto/subtle"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"sort"
	"strings"
	"sync"
	"time"
)

const (
	moduleID = "social-uploader"
	maxBody  = 1 << 20
)

type JobStatus string

const (
	StatusQueued     JobStatus = "queued"
	StatusPublishing JobStatus = "publishing"
	StatusSucceeded  JobStatus = "succeeded"
	StatusFailed     JobStatus = "failed"
	StatusUnknown    JobStatus = "unknown"
)

type Job struct {
	ID             string         \`json:"id"\`
	ProjectID      string         \`json:"project_id"\`
	Platform       string         \`json:"platform"\`
	AccountID      string         \`json:"account_id"\`
	MediaRef       string         \`json:"media_ref"\`
	Payload        map[string]any \`json:"payload"\`
	IdempotencyKey string         \`json:"idempotency_key"\`
	Status         JobStatus      \`json:"status"\`
	ErrorCode      string         \`json:"error_code,omitempty"\`
	CreatedAt      time.Time      \`json:"created_at"\`
	UpdatedAt      time.Time      \`json:"updated_at"\`
}

type CreateJobRequest struct {
	Platform       string         \`json:"platform"\`
	AccountID      string         \`json:"account_id"\`
	MediaRef       string         \`json:"media_ref"\`
	Payload        map[string]any \`json:"payload"\`
	IdempotencyKey string         \`json:"idempotency_key"\`
}

type Store struct {
	mu   sync.RWMutex
	path string
	jobs map[string]Job
}

var (
	idPattern       = regexp.MustCompile(\`^[a-zA-Z0-9][a-zA-Z0-9._:-]{0,127}$\`)
	errorCodePattern = regexp.MustCompile(\`^[a-z0-9_]{1,80}$\`)
	projectTokens   map[string]string
	projectAccounts map[string]map[string]bool
	supportedTarget = map[string]bool{"tiktok": true, "instagram": true, "youtube": true, "facebook": true, "x": true, "linkedin": true}
)

func openStore(path string) (*Store, error) {
	if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
		return nil, err
	}
	s := &Store{path: path, jobs: map[string]Job{}}
	b, err := os.ReadFile(path)
	if err != nil && !errors.Is(err, os.ErrNotExist) {
		return nil, err
	}
	if len(b) > 0 {
		if err := json.Unmarshal(b, &s.jobs); err != nil {
			return nil, fmt.Errorf("decode queue store: %w", err)
		}
	}
	for id, job := range s.jobs {
		if job.Status == StatusPublishing {
			job.Status = StatusUnknown
			job.ErrorCode = "publish_outcome_unknown_after_restart"
			job.UpdatedAt = time.Now().UTC()
			s.jobs[id] = job
		}
	}
	if err := s.persistLocked(); err != nil {
		return nil, err
	}
	return s, nil
}

func (s *Store) persistLocked() error {
	b, err := json.MarshalIndent(s.jobs, "", "  ")
	if err != nil {
		return err
	}
	tmp := s.path + ".tmp"
	f, err := os.OpenFile(tmp, os.O_CREATE|os.O_TRUNC|os.O_WRONLY, 0600)
	if err != nil {
		return err
	}
	if _, err = f.Write(append(b, '\n')); err != nil {
		_ = f.Close()
		return err
	}
	if err = f.Sync(); err != nil {
		_ = f.Close()
		return err
	}
	if err = f.Close(); err != nil {
		return err
	}
	if err = os.Rename(tmp, s.path); err != nil {
		return err
	}
	if d, e := os.Open(filepath.Dir(s.path)); e == nil {
		_ = d.Sync()
		_ = d.Close()
	}
	return nil
}

func (s *Store) create(project string, req CreateJobRequest) (Job, bool, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, existing := range s.jobs {
		if existing.ProjectID == project && existing.IdempotencyKey == req.IdempotencyKey {
			return existing, false, nil
		}
	}
	now := time.Now().UTC()
	job := Job{
		ID: fmt.Sprintf("suj_%d", now.UnixNano()), ProjectID: project,
		Platform: req.Platform, AccountID: req.AccountID, MediaRef: req.MediaRef,
		Payload: req.Payload, IdempotencyKey: req.IdempotencyKey,
		Status: StatusQueued, CreatedAt: now, UpdatedAt: now,
	}
	s.jobs[job.ID] = job
	if err := s.persistLocked(); err != nil {
		delete(s.jobs, job.ID)
		return Job{}, false, err
	}
	return job, true, nil
}

func (s *Store) list(project string) []Job {
	s.mu.RLock()
	defer s.mu.RUnlock()
	out := make([]Job, 0)
	for _, j := range s.jobs {
		if j.ProjectID == project {
			out = append(out, j)
		}
	}
	sort.Slice(out, func(i, j int) bool { return out[i].CreatedAt.After(out[j].CreatedAt) })
	return out
}

func (s *Store) get(project, id string) (Job, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	j, ok := s.jobs[id]
	return j, ok && j.ProjectID == project
}

// claim is atomic: only a queued job can start, so repeated publish requests cannot
// launch multiple browser sessions for the same job.
func (s *Store) claim(project, id string) (Job, bool, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	job, ok := s.jobs[id]
	if !ok || job.ProjectID != project || job.Status != StatusQueued {
		return Job{}, false, nil
	}
	job.Status = StatusPublishing
	job.ErrorCode = ""
	job.UpdatedAt = time.Now().UTC()
	s.jobs[id] = job
	if err := s.persistLocked(); err != nil {
		job.Status = StatusQueued
		job.UpdatedAt = time.Now().UTC()
		s.jobs[id] = job
		return Job{}, false, err
	}
	return job, true, nil
}

func (s *Store) finish(id string, status JobStatus, errorCode string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	job, ok := s.jobs[id]
	if !ok {
		return errors.New("job disappeared")
	}
	job.Status = status
	job.ErrorCode = errorCode
	job.UpdatedAt = time.Now().UTC()
	s.jobs[id] = job
	return s.persistLocked()
}

type API struct{ store *Store }

func (a *API) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("X-Content-Type-Options", "nosniff")
	w.Header().Set("Cache-Control", "no-store")
	if r.Method == http.MethodGet && r.URL.Path == "/health" {
		writeJSON(w, http.StatusOK, map[string]any{"status": "ok", "module": moduleID})
		return
	}
	if !strings.HasPrefix(r.URL.Path, "/api/v1/social-uploader/") {
		writeError(w, http.StatusNotFound, "not_found", "route not found")
		return
	}
	parts := strings.Split(strings.Trim(r.URL.Path, "/"), "/")
	if len(parts) < 6 || parts[0] != "api" || parts[1] != "v1" || parts[2] != "social-uploader" || parts[3] != "projects" {
		writeError(w, http.StatusNotFound, "not_found", "route not found")
		return
	}
	project := parts[4]
	if !idPattern.MatchString(project) {
		writeError(w, http.StatusBadRequest, "invalid_project_id", "invalid project id")
		return
	}
	if !a.authorize(w, r, project) {
		return
	}
	if len(parts) == 6 && parts[5] == "jobs" {
		switch r.Method {
		case http.MethodPost:
			a.createJob(w, r, project)
		case http.MethodGet:
			writeJSON(w, http.StatusOK, map[string]any{"jobs": a.store.list(project)})
		default:
			w.Header().Set("Allow", "GET, POST")
			writeError(w, http.StatusMethodNotAllowed, "method_not_allowed", "method not allowed")
		}
		return
	}
	if len(parts) == 7 && parts[5] == "jobs" {
		jobID := parts[6]
		if r.Method == http.MethodGet {
			job, ok := a.store.get(project, jobID)
			if !ok {
				writeError(w, http.StatusNotFound, "job_not_found", "job not found")
				return
			}
			writeJSON(w, http.StatusOK, job)
			return
		}
		if r.Method == http.MethodPost && strings.HasSuffix(jobID, ":publish") {
			a.publish(w, project, strings.TrimSuffix(jobID, ":publish"))
			return
		}
	}
	writeError(w, http.StatusNotFound, "not_found", "route not found")
}

func (a *API) authorize(w http.ResponseWriter, r *http.Request, project string) bool {
	provided := r.Header.Get("X-Social-Uploader-Project-Token")
	if provided == "" {
		writeError(w, http.StatusUnauthorized, "unauthorized", "project token required")
		return false
	}
	expected, ok := projectTokens[project]
	if !ok || subtle.ConstantTimeCompare([]byte(provided), []byte(expected)) != 1 {
		writeError(w, http.StatusForbidden, "project_access_denied", "token is not authorized for this project")
		return false
	}
	return true
}

func (a *API) createJob(w http.ResponseWriter, r *http.Request, project string) {
	r.Body = http.MaxBytesReader(w, r.Body, maxBody)
	defer r.Body.Close()
	dec := json.NewDecoder(r.Body)
	dec.DisallowUnknownFields()
	var req CreateJobRequest
	if err := dec.Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid_json", "invalid request body")
		return
	}
	if err := dec.Decode(&struct{}{}); err != io.EOF {
		writeError(w, http.StatusBadRequest, "invalid_json", "request must contain one JSON object")
		return
	}
	req.Platform = strings.ToLower(strings.TrimSpace(req.Platform))
	req.AccountID = strings.TrimSpace(req.AccountID)
	req.MediaRef = strings.TrimSpace(req.MediaRef)
	req.IdempotencyKey = strings.TrimSpace(req.IdempotencyKey)
	if !supportedTarget[req.Platform] {
		writeError(w, http.StatusBadRequest, "unsupported_platform", "unsupported platform")
		return
	}
	if !projectAccounts[project][req.AccountID] {
		writeError(w, http.StatusForbidden, "account_not_assigned", "target account is not assigned to this project")
		return
	}
	if !idPattern.MatchString(req.AccountID) || !idPattern.MatchString(req.MediaRef) || !idPattern.MatchString(req.IdempotencyKey) {
		writeError(w, http.StatusBadRequest, "invalid_fields", "account_id, media_ref and idempotency_key must be valid identifiers")
		return
	}
	if req.Payload == nil {
		writeError(w, http.StatusBadRequest, "invalid_payload", "payload snapshot is required")
		return
	}
	job, created, err := a.store.create(project, req)
	if err != nil {
		log.Printf("queue persistence failed: %v", err)
		writeError(w, http.StatusInternalServerError, "persistence_failed", "could not persist publishing job")
		return
	}
	status := http.StatusCreated
	if !created {
		status = http.StatusOK
	}
	writeJSON(w, status, job)
}

func (a *API) publish(w http.ResponseWriter, project, id string) {
	if id == "" || !idPattern.MatchString(id) {
		writeError(w, http.StatusBadRequest, "invalid_job_id", "invalid job id")
		return
	}
	job, exists := a.store.get(project, id)
	if !exists {
		writeError(w, http.StatusNotFound, "job_not_found", "job not found")
		return
	}
	if job.Platform != "tiktok" {
		writeError(w, http.StatusNotImplemented, "adapter_unavailable", "only the TikTok browser adapter is implemented")
		return
	}
	claimed, ok, err := a.store.claim(project, id)
	if err != nil {
		log.Printf("could not claim publishing job: %v", err)
		writeError(w, http.StatusInternalServerError, "persistence_failed", "could not start publishing job")
		return
	}
	if !ok {
		writeError(w, http.StatusConflict, "job_not_queued", "job is not queued; check its current status before taking further action")
		return
	}
	go a.runTikTok(claimed)
	w.WriteHeader(http.StatusAccepted)
	_ = json.NewEncoder(w).Encode(claimed)
}

type workerResult struct {
	Status    string \`json:"status"\`
	ErrorCode string \`json:"error_code"\`
}

func (a *API) runTikTok(job Job) {
	mediaRoot := os.Getenv("SOCIAL_UPLOADER_MEDIA_ROOT")
	if mediaRoot == "" {
		mediaRoot = filepath.Join("data", "modules", moduleID, "media")
	}
	mediaRoot, err := filepath.Abs(mediaRoot)
	if err != nil {
		_ = a.store.finish(job.ID, StatusFailed, "invalid_media_root")
		return
	}
	mediaName := filepath.Base(job.MediaRef)
	if mediaName != job.MediaRef || strings.Contains(job.MediaRef, string(os.PathSeparator)) || strings.Contains(job.MediaRef, "/") || strings.Contains(job.MediaRef, "\\") {
		_ = a.store.finish(job.ID, StatusFailed, "invalid_media_ref")
		return
	}
	mediaPath := filepath.Join(mediaRoot, mediaName)
	rel, err := filepath.Rel(mediaRoot, mediaPath)
	if err != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(os.PathSeparator)) {
		_ = a.store.finish(job.ID, StatusFailed, "invalid_media_ref")
		return
	}
	info, err := os.Stat(mediaPath)
	if err != nil || !info.Mode().IsRegular() {
		_ = a.store.finish(job.ID, StatusFailed, "media_not_found")
		return
	}
	caption, ok := job.Payload["caption"].(string)
	if !ok || strings.TrimSpace(caption) == "" || len(caption) > 2200 {
		_ = a.store.finish(job.ID, StatusFailed, "invalid_caption")
		return
	}

	profileRoot := os.Getenv("SOCIAL_UPLOADER_PROFILE_ROOT")
	if profileRoot == "" {
		profileRoot = filepath.Join("data", "modules", moduleID, "profiles")
	}
	profilePath := filepath.Join(profileRoot, job.ProjectID, job.AccountID)
	if err := os.MkdirAll(profilePath, 0700); err != nil {
		_ = a.store.finish(job.ID, StatusFailed, "profile_directory_failed")
		return
	}
	worker := os.Getenv("SOCIAL_UPLOADER_TIKTOK_WORKER")
	if worker == "" {
		if executable, e := os.Executable(); e == nil {
			candidate := filepath.Join(filepath.Dir(executable), "tiktok_worker.py")
			if _, e := os.Stat(candidate); e == nil {
				worker = candidate
			}
		}
	}
	if worker == "" {
		worker = filepath.Join("modules", moduleID, "tiktok_worker.py")
		if _, e := os.Stat(worker); e != nil {
			worker = "tiktok_worker.py"
		}
	}
	python := os.Getenv("SOCIAL_UPLOADER_PYTHON")
	if python == "" {
		if runtime.GOOS == "windows" {
			python = "python"
		} else {
			python = "python3"
		}
	}
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Minute)
	defer cancel()
	cmd := exec.CommandContext(ctx, python, worker,
		"--media", mediaPath,
		"--profile", profilePath,
		"--caption", caption,
	)
	var stdout strings.Builder
	cmd.Stdout = &stdout
	// Do not forward browser stderr to logs; it can include account or page details.
	cmd.Stderr = io.Discard
	err = cmd.Run()
	if ctx.Err() != nil {
		_ = a.store.finish(job.ID, StatusUnknown, "worker_timeout_outcome_unknown")
		return
	}
	var result workerResult
	if json.Unmarshal([]byte(strings.TrimSpace(stdout.String())), &result) != nil {
		if err != nil {
			_ = a.store.finish(job.ID, StatusFailed, "worker_execution_failed")
		} else {
			_ = a.store.finish(job.ID, StatusUnknown, "worker_returned_invalid_result")
		}
		return
	}
	switch result.Status {
	case "succeeded":
		_ = a.store.finish(job.ID, StatusSucceeded, "")
	case "failed":
		code := result.ErrorCode
		if !errorCodePattern.MatchString(code) {
			code = "worker_failed"
		}
		_ = a.store.finish(job.ID, StatusFailed, code)
	default:
		code := result.ErrorCode
		if !errorCodePattern.MatchString(code) {
			code = "publish_outcome_unknown"
		}
		_ = a.store.finish(job.ID, StatusUnknown, code)
	}
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}
func writeError(w http.ResponseWriter, status int, code, message string) {
	writeJSON(w, status, map[string]string{"error": code, "message": message})
}

func loadProjectTokens() (map[string]string, error) {
	raw := os.Getenv("SOCIAL_UPLOADER_PROJECT_TOKENS")
	if raw == "" {
		return nil, errors.New("SOCIAL_UPLOADER_PROJECT_TOKENS must be a JSON object mapping project IDs to unique bearer tokens")
	}
	var tokens map[string]string
	if err := json.Unmarshal([]byte(raw), &tokens); err != nil {
		return nil, fmt.Errorf("parse SOCIAL_UPLOADER_PROJECT_TOKENS: %w", err)
	}
	if len(tokens) == 0 {
		return nil, errors.New("at least one project token is required")
	}
	for project, token := range tokens {
		if !idPattern.MatchString(project) || len(token) < 32 {
			return nil, fmt.Errorf("project %q has invalid id or token shorter than 32 characters", project)
		}
	}
	return tokens, nil
}

func loadProjectAccounts() (map[string]map[string]bool, error) {
	raw := os.Getenv("SOCIAL_UPLOADER_PROJECT_ACCOUNTS")
	if raw == "" {
		return nil, errors.New("SOCIAL_UPLOADER_PROJECT_ACCOUNTS must map each project ID to its allowed account IDs")
	}
	var source map[string][]string
	if err := json.Unmarshal([]byte(raw), &source); err != nil {
		return nil, fmt.Errorf("parse SOCIAL_UPLOADER_PROJECT_ACCOUNTS: %w", err)
	}
	accounts := make(map[string]map[string]bool, len(source))
	for project, ids := range source {
		if !idPattern.MatchString(project) || len(ids) == 0 {
			return nil, fmt.Errorf("project %q must have a valid id and at least one account", project)
		}
		accounts[project] = make(map[string]bool, len(ids))
		for _, id := range ids {
			if !idPattern.MatchString(id) {
				return nil, fmt.Errorf("project %q has invalid account id %q", project, id)
			}
			accounts[project][id] = true
		}
	}
	return accounts, nil
}

func main() {
	var err error
	projectTokens, err = loadProjectTokens()
	if err != nil {
		log.Fatal(err)
	}
	projectAccounts, err = loadProjectAccounts()
	if err != nil {
		log.Fatal(err)
	}
	for project := range projectTokens {
		if _, ok := projectAccounts[project]; !ok {
			log.Fatalf("project %q has a token but no account allowlist", project)
		}
	}
	for project := range projectAccounts {
		if _, ok := projectTokens[project]; !ok {
			log.Fatalf("project %q has an account allowlist but no token", project)
		}
	}
	path := os.Getenv("SOCIAL_UPLOADER_DATA_FILE")
	if path == "" {
		path = filepath.Join("data", "modules", moduleID, "jobs.json")
	}
	store, err := openStore(path)
	if err != nil {
		log.Fatalf("open queue store: %v", err)
	}
	port := strings.TrimSpace(os.Getenv("MLHSM_MODULE_PORT"))
	if port == "" && len(os.Args) > 1 {
		port = os.Args[1]
	}
	if port == "" {
		log.Fatal("MLHSM_MODULE_PORT is required")
	}
	if strings.Contains(port, ":") {
		log.Fatal("MLHSM_MODULE_PORT must be a port number, not a host:port")
	}
	listener, err := net.Listen("tcp", net.JoinHostPort("127.0.0.1", port))
	if err != nil {
		log.Fatalf("listen: %v", err)
	}
	server := &http.Server{Handler: &API{store: store}, ReadHeaderTimeout: 5 * time.Second, ReadTimeout: 15 * time.Second, WriteTimeout: 15 * time.Second, IdleTimeout: 60 * time.Second}
	log.Printf("%s listening on loopback port %s", moduleID, port)
	if err := server.Serve(listener); err != nil && err != http.ErrServerClosed {
		log.Fatal(err)
	}
}
