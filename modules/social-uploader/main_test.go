package main

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"testing"
)

func testAPI(t *testing.T) *API {
	t.Helper()
	store, err := openStore(filepath.Join(t.TempDir(), "jobs.json"))
	if err != nil { t.Fatal(err) }
	projectTokens = map[string]string{"rapitup":"rapitup-test-token-0123456789012345", "lyricamundo":"lyricamundo-test-token-012345678901"}
	projectAccounts = map[string]map[string]bool{"rapitup":{"account-1":true}, "lyricamundo":{"account-2":true}}
	return &API{store:store}
}

func TestCreateJobIsIdempotentAndProjectScoped(t *testing.T) {
	api := testAPI(t)
	body := CreateJobRequest{Platform:"tiktok", AccountID:"account-1", MediaRef:"asset-1", Payload:map[string]any{"caption":"test"}, IdempotencyKey:"request-1"}
	raw, _ := json.Marshal(body)
	call := func() *httptest.ResponseRecorder {
		r := httptest.NewRequest(http.MethodPost, "/api/v1/social-uploader/projects/rapitup/jobs", bytes.NewReader(raw))
		r.Header.Set("X-Social-Uploader-Project-Token", "rapitup-test-token-0123456789012345")
		w := httptest.NewRecorder(); api.ServeHTTP(w,r); return w
	}
	first := call()
	if first.Code != http.StatusCreated { t.Fatalf("first status = %d, body %s", first.Code, first.Body.String()) }
	second := call()
	if second.Code != http.StatusOK { t.Fatalf("duplicate status = %d", second.Code) }
	var a, b Job
	if err := json.Unmarshal(first.Body.Bytes(), &a); err != nil { t.Fatal(err) }
	if err := json.Unmarshal(second.Body.Bytes(), &b); err != nil { t.Fatal(err) }
	if a.ID != b.ID { t.Fatalf("idempotency mismatch: %s != %s", a.ID,b.ID) }
	if len(api.store.list("lyricamundo")) != 0 { t.Fatal("job leaked across projects") }
	wrongAccount := body; wrongAccount.AccountID = "account-2"; wrongRaw, _ := json.Marshal(wrongAccount)
	r := httptest.NewRequest(http.MethodPost, "/api/v1/social-uploader/projects/rapitup/jobs", bytes.NewReader(wrongRaw)); r.Header.Set("X-Social-Uploader-Project-Token", "rapitup-test-token-0123456789012345")
	w := httptest.NewRecorder(); api.ServeHTTP(w,r); if w.Code != http.StatusForbidden { t.Fatalf("cross-project account status = %d", w.Code) }
}

func TestProjectTokenCannotReadAnotherProject(t *testing.T) {
	api := testAPI(t)
	r := httptest.NewRequest(http.MethodGet, "/api/v1/social-uploader/projects/lyricamundo/jobs", nil)
	r.Header.Set("Authorization", "Bearer rapitup-test-token-0123456789012345")
	w := httptest.NewRecorder(); api.ServeHTTP(w,r)
	if w.Code != http.StatusForbidden { t.Fatalf("status = %d, want 403", w.Code) }
}

func TestPublishingFailsClosedWithoutAdapter(t *testing.T) {
	api := testAPI(t)
	r := httptest.NewRequest(http.MethodPost, "/api/v1/social-uploader/projects/rapitup/jobs/suj_123:publish", nil)
	r.Header.Set("Authorization", "Bearer rapitup-test-token-0123456789012345")
	w := httptest.NewRecorder(); api.ServeHTTP(w,r)
	if w.Code != http.StatusNotImplemented { t.Fatalf("status = %d, want 501", w.Code) }
}

func TestPublishingStateBecomesUnknownOnRestart(t *testing.T) {
	path := filepath.Join(t.TempDir(), "jobs.json")
	store, err := openStore(path); if err != nil { t.Fatal(err) }
	job := Job{ID:"suj_1", ProjectID:"rapitup", Status:StatusPublishing}
	store.mu.Lock(); store.jobs[job.ID] = job; err = store.persistLocked(); store.mu.Unlock()
	if err != nil { t.Fatal(err) }
	reopened, err := openStore(path); if err != nil { t.Fatal(err) }
	got, ok := reopened.get("rapitup","suj_1")
	if !ok || got.Status != StatusUnknown { t.Fatalf("got status %q, ok=%v", got.Status,ok) }
}
