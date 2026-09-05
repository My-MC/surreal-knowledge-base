//! Auth + blog integration tests (plan todo 7): register/login with JWT
//! cookies, the blog upload guard, publish visibility, and the PUT/DELETE
//! blog_post consistency. Serial execution is required: `SKB_SERVER_JWT_SECRET`
//! mutation is process-global and the embedded store takes a cross-process
//! lock.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

use common::{test_router, test_state};

const SECRET: &str = "test-secret-0123456789abcdef-0123456789abcdef";

/// Restores the previous values of the touched env keys on drop (the
/// config.rs EnvGuard pattern). Guards compose — one per key.
struct EnvGuard(Vec<(&'static str, Option<String>)>);

impl EnvGuard {
    fn set(value: &str) -> Self {
        Self::set_key("SKB_SERVER_JWT_SECRET", value)
    }

    fn remove() -> Self {
        Self::remove_key("SKB_SERVER_JWT_SECRET")
    }

    fn set_key(key: &'static str, value: &str) -> Self {
        let old = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self(vec![(key, old)])
    }

    fn remove_key(key: &'static str) -> Self {
        let old = std::env::var(key).ok();
        std::env::remove_var(key);
        Self(vec![(key, old)])
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, old) in self.0.drain(..) {
            match old {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

struct TestResponse {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: Value,
}

/// Drive one request through the in-process router with extra request
/// headers (common::send cannot carry the Cookie header or expose
/// Set-Cookie).
async fn send(
    router: axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    headers: &[(&str, String)],
) -> TestResponse {
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, value);
    }
    let request = match body {
        Some(value) => builder
            .header("content-type", "application/json")
            .body(Body::from(value.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = router.oneshot(request).await.unwrap();
    let status = response.status();
    let response_headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    TestResponse {
        status,
        headers: response_headers,
        body,
    }
}

async fn setup() -> (axum::Router, std::path::PathBuf) {
    let (state, db_path) = test_state().await;
    (test_router(state), db_path)
}

async fn register(router: &axum::Router, email: &str, password: &str) -> TestResponse {
    send(
        router.clone(),
        "POST",
        "/api/auth/register",
        Some(json!({"email": email, "password": password})),
        &[],
    )
    .await
}

async fn login(router: &axum::Router, email: &str, password: &str) -> TestResponse {
    send(
        router.clone(),
        "POST",
        "/api/auth/login",
        Some(json!({"email": email, "password": password})),
        &[],
    )
    .await
}

/// The `skb_session=<jwt>` pair from the login response's Set-Cookie header.
fn session_cookie(response: &TestResponse) -> String {
    let raw = response
        .headers
        .get(header::SET_COOKIE)
        .unwrap_or_else(|| panic!("no Set-Cookie header in response"))
        .to_str()
        .unwrap();
    assert!(raw.contains("HttpOnly"), "cookie must be HttpOnly: {raw}");
    assert!(
        raw.contains("SameSite=Lax"),
        "cookie must be SameSite=Lax: {raw}"
    );
    raw.split(';').next().unwrap().to_string()
}

async fn upload_blog(
    router: &axum::Router,
    cookie: Option<&str>,
    content: &str,
    title: &str,
) -> TestResponse {
    let headers: Vec<(&str, String)> = cookie.iter().map(|c| ("cookie", c.to_string())).collect();
    send(
        router.clone(),
        "POST",
        "/api/documents",
        Some(json!({
            "content": content,
            "title": title,
            "metadata": {"app": "blog"}
        })),
        &headers,
    )
    .await
}

async fn publish(router: &axum::Router, document_id: &str, cookie: Option<&str>) -> TestResponse {
    let headers: Vec<(&str, String)> = cookie.iter().map(|c| ("cookie", c.to_string())).collect();
    send(
        router.clone(),
        "POST",
        &format!("/api/blog/posts/{document_id}/publish"),
        None,
        &headers,
    )
    .await
}

async fn list_posts(router: &axum::Router) -> TestResponse {
    send(router.clone(), "GET", "/api/blog/posts", None, &[]).await
}

/// Given: an author with a session cookie.
/// When:  uploading a blog document, publishing it, and listing posts.
/// Then:  the post is hidden until publish and listed with full metadata
///        (document_id, title, created_at, author email) afterwards.
#[tokio::test]
async fn blog_full_flow_hides_until_publish_then_lists() {
    let _secret = EnvGuard::set(SECRET);
    let _authors = EnvGuard::set_key("SKB_SERVER_AUTHOR_EMAILS", "author@example.com");
    let (router, _db) = setup().await;

    let reg = register(&router, "author@example.com", "pw-author").await;
    assert_eq!(reg.status, StatusCode::CREATED, "register: {}", reg.body);
    assert_eq!(reg.body["email"], "author@example.com");
    assert_eq!(reg.body["role"], "author");

    let login = login(&router, "author@example.com", "pw-author").await;
    assert_eq!(login.status, StatusCode::OK, "login: {}", login.body);
    assert_eq!(login.body["role"], "author");
    let cookie = session_cookie(&login);

    let upload = upload_blog(&router, Some(&cookie), "hello blog world", "Blog One").await;
    assert_eq!(
        upload.status,
        StatusCode::CREATED,
        "upload: {}",
        upload.body
    );
    let document_id = upload.body["document_id"].as_str().unwrap().to_string();

    let hidden = list_posts(&router).await;
    assert_eq!(hidden.status, StatusCode::OK);
    assert_eq!(hidden.body, json!([]), "unpublished post must be hidden");

    let publish = publish(&router, &document_id, Some(&cookie)).await;
    assert_eq!(publish.status, StatusCode::OK, "publish: {}", publish.body);
    assert_eq!(publish.body["published"], json!(true));

    let listed = list_posts(&router).await;
    assert_eq!(listed.status, StatusCode::OK);
    let posts = listed.body.as_array().unwrap();
    assert_eq!(posts.len(), 1, "posts: {}", listed.body);
    assert_eq!(posts[0]["document_id"], json!(document_id));
    assert_eq!(posts[0]["title"], "Blog One");
    assert_eq!(posts[0]["author"], "author@example.com");
    assert!(!posts[0]["created_at"].as_str().unwrap().is_empty());
}

/// Given: a registered reader with a valid session cookie.
/// When:  uploading a document marked app=blog.
/// Then:  401 E_VALIDATION (blog uploads are author-only).
#[tokio::test]
async fn reader_cookie_is_unauthorized_for_blog_upload() {
    let _secret = EnvGuard::set(SECRET);
    let _no_authors = EnvGuard::remove_key("SKB_SERVER_AUTHOR_EMAILS");
    let (router, _db) = setup().await;

    let reg = register(&router, "reader@example.com", "pw-reader").await;
    assert_eq!(reg.status, StatusCode::CREATED);
    let login = login(&router, "reader@example.com", "pw-reader").await;
    assert_eq!(login.body["role"], "reader");
    let cookie = session_cookie(&login);

    let upload = upload_blog(&router, Some(&cookie), "reader content", "Reader Post").await;
    assert_eq!(
        upload.status,
        StatusCode::UNAUTHORIZED,
        "upload: {}",
        upload.body
    );
    assert_eq!(upload.body["code"], "E_VALIDATION");
}

/// Given: no session cookie.
/// When:  uploading a blog document or publishing.
/// Then:  both are 401.
#[tokio::test]
async fn missing_token_is_unauthorized_for_blog_upload_and_publish() {
    let _secret = EnvGuard::set(SECRET);
    let _authors = EnvGuard::set_key("SKB_SERVER_AUTHOR_EMAILS", "author@example.com");
    let (router, _db) = setup().await;

    let upload = upload_blog(&router, None, "anon content", "Anon Post").await;
    assert_eq!(
        upload.status,
        StatusCode::UNAUTHORIZED,
        "upload: {}",
        upload.body
    );

    register(&router, "author@example.com", "pw").await;
    let login = login(&router, "author@example.com", "pw").await;
    let cookie = session_cookie(&login);
    let upload = upload_blog(&router, Some(&cookie), "content", "T").await;
    let document_id = upload.body["document_id"].as_str().unwrap().to_string();

    let publish = publish(&router, &document_id, None).await;
    assert_eq!(
        publish.status,
        StatusCode::UNAUTHORIZED,
        "publish: {}",
        publish.body
    );
}

/// Given: SKB_SERVER_JWT_SECRET is unset.
/// When:  uploading a blog document / listing posts.
/// Then:  the auth-requiring path is 503 E_CONFIG while the public listing
///        stays 200.
#[tokio::test]
async fn unset_jwt_secret_degrades_auth_paths_but_not_public_routes() {
    let _secret = EnvGuard::remove();
    let (router, _db) = setup().await;

    let upload = upload_blog(&router, None, "content", "T").await;
    assert_eq!(
        upload.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "upload: {}",
        upload.body
    );
    assert_eq!(upload.body["code"], "E_CONFIG");

    let posts = list_posts(&router).await;
    assert_eq!(posts.status, StatusCode::OK);
    assert_eq!(posts.body, json!([]));
}

/// Given: SKB_SERVER_JWT_SECRET is set but shorter than 32 characters.
/// When:  calling an authenticated path.
/// Then:  503 E_CONFIG — weak secrets must not back HS256 sessions.
#[tokio::test]
async fn weak_jwt_secret_is_rejected_like_an_unset_one() {
    let _secret = EnvGuard::set("too-short");
    let (router, _db) = setup().await;

    let login = login(&router, "nobody@example.com", "pw").await;
    assert_eq!(login.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(login.body["code"], "E_CONFIG");
}

/// Given: a registered user.
/// When:  logging in with a wrong password, logging in as an unknown user,
///        or registering a duplicate email.
/// Then:  wrong/unknown credentials are a generic 401; the duplicate is 409.
#[tokio::test]
async fn wrong_password_is_401_and_duplicate_email_is_409() {
    let _secret = EnvGuard::set(SECRET);
    let _no_authors = EnvGuard::remove_key("SKB_SERVER_AUTHOR_EMAILS");
    let (router, _db) = setup().await;

    let reg = register(&router, "dup@example.com", "pw").await;
    assert_eq!(reg.status, StatusCode::CREATED);

    let bad = login(&router, "dup@example.com", "wrong-password").await;
    assert_eq!(
        bad.status,
        StatusCode::UNAUTHORIZED,
        "bad login: {}",
        bad.body
    );
    assert_eq!(bad.body["code"], "E_VALIDATION");

    let unknown = login(&router, "ghost@example.com", "pw").await;
    assert_eq!(unknown.status, StatusCode::UNAUTHORIZED);

    let dup = register(&router, "dup@example.com", "other").await;
    assert_eq!(dup.status, StatusCode::CONFLICT, "dup: {}", dup.body);
    assert_eq!(dup.body["code"], "E_VALIDATION");
}

/// Given: a published blog document.
/// When:  PUT with changed content mints a new id, then DELETE removes the
///        document.
/// Then:  the blog_post migrates to the new document id and disappears with
///        the delete.
#[tokio::test]
async fn put_migrates_blog_post_and_delete_removes_it() {
    let _secret = EnvGuard::set(SECRET);
    let _authors = EnvGuard::set_key("SKB_SERVER_AUTHOR_EMAILS", "author@example.com");
    let (router, _db) = setup().await;

    register(&router, "author@example.com", "pw").await;
    let login = login(&router, "author@example.com", "pw").await;
    let cookie = session_cookie(&login);

    let upload = upload_blog(&router, Some(&cookie), "version one", "Migrated Post").await;
    let old_id = upload.body["document_id"].as_str().unwrap().to_string();
    let publish = publish(&router, &old_id, Some(&cookie)).await;
    assert_eq!(publish.status, StatusCode::OK);

    let put = send(
        router.clone(),
        "PUT",
        &format!("/api/documents/{old_id}"),
        Some(json!({"content": "version two", "title": "Migrated Post"})),
        &[],
    )
    .await;
    assert_eq!(put.status, StatusCode::OK, "put: {}", put.body);
    let new_id = put.body["document_id"].as_str().unwrap().to_string();
    assert_ne!(new_id, old_id, "changed content must mint a new id");

    let listed = list_posts(&router).await;
    let posts = listed.body.as_array().unwrap();
    assert_eq!(posts.len(), 1, "posts after PUT: {}", listed.body);
    assert_eq!(posts[0]["document_id"], json!(new_id));

    let del = send(
        router.clone(),
        "DELETE",
        &format!("/api/documents/{new_id}"),
        None,
        &[],
    )
    .await;
    assert_eq!(del.status, StatusCode::NO_CONTENT, "delete: {}", del.body);

    let listed = list_posts(&router).await;
    assert_eq!(
        listed.body,
        json!([]),
        "posts after DELETE: {}",
        listed.body
    );
}
