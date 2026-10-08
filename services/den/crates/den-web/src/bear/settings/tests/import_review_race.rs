use super::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Condvar, Mutex,
};

async fn upload_only(app: &axum::Router, cookie: &str, bytes: &[u8]) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/bears/import")
                .header(header::COOKIE, cookie)
                .header(
                    header::CONTENT_TYPE,
                    "multipart/form-data; boundary=portable",
                )
                .body(Body::from(portability::upload(bytes, false)))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_review_get_cannot_overwrite_a_replacement_uploads_session_pointer() {
    let _guard = TEST_DB_LOCK.lock().await;
    let Some(pool) = test_pool().await else {
        return;
    };
    let slug = fresh_slug();
    let bear_id = create_test_bear(&pool, &slug).await;
    let actor = create_bear_admin_user(&pool, bear_id).await;
    let bear = bears_db::get_bear(&pool, bear_id).await.unwrap().unwrap();
    let mut manifest = manifest_for_bear(&bear).unwrap();
    manifest.bear.name = "Upload A".into();
    let a = build_bear_bundle(
        &serde_yml::to_string(&manifest).unwrap(),
        b"SQLite placeholder",
    )
    .unwrap();
    manifest.bear.name = "Replacement B".into();
    let b = build_bear_bundle(
        &serde_yml::to_string(&manifest).unwrap(),
        b"SQLite placeholder",
    )
    .unwrap();
    let target: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let pause_once = Arc::new(AtomicBool::new(true));
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let (entered, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut state = test_state(pool.clone());
    {
        let (target, pause_once, release) = (target.clone(), pause_once.clone(), release.clone());
        state
            .template_env
            .add_function("import_review_test_gate", move |nonce: String| {
                let should_pause = target.lock().unwrap().as_deref() == Some(nonce.as_str());
                if should_pause && pause_once.swap(false, Ordering::SeqCst) {
                    entered.send(()).unwrap();
                    let (lock, ready) = &*release;
                    let mut released = lock.lock().unwrap();
                    while !*released {
                        released = ready.wait(released).unwrap();
                    }
                }
                String::new()
            });
    }
    let template = include_str!("../../../templates/bear/manage/import_review.html").replace(
        "{% block content %}",
        "{% block content %}{{ import_review_test_gate(nonce) }}",
    );
    state
        .template_env
        .add_template_owned("bear/manage/import_review.html".to_string(), template)
        .unwrap();
    let app = test_app_with_state(pool.clone(), state).await;
    let cookie = login_cookie(&app, actor).await;
    let uploaded = upload_only(&app, &cookie, &a).await;
    assert_eq!(uploaded.status(), StatusCode::SEE_OTHER);
    let review_a = uploaded.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned();
    *target.lock().unwrap() = Some(review_a.rsplit('/').next().unwrap().into());
    let old_get = {
        let app = app.clone();
        let cookie = cookie.clone();
        tokio::spawn(async move {
            app.oneshot(
                Request::builder()
                    .uri(review_a)
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(2), entered_rx.recv())
        .await
        .unwrap()
        .unwrap();
    // GET A has already loaded its old session snapshot and verified its file,
    // but has not finished rendering. Complete replacement POST B first.
    let replacement = upload_only(&app, &cookie, &b).await;
    {
        let (lock, ready) = &*release;
        *lock.lock().unwrap() = true;
        ready.notify_all();
    }
    let old_response = old_get.await.unwrap();
    assert_eq!(replacement.status(), StatusCode::SEE_OTHER);
    assert_eq!(old_response.status(), StatusCode::BAD_REQUEST);
    let review_b = replacement.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned();
    // B's receipt cannot be inherited from A's GET, even after A finishes late.
    assert_eq!(
        portability::confirm(&app, &cookie, &review_b, true)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let (status, page) = get_as(&app, &cookie, &review_b).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains("Replacement B"));
    assert_eq!(
        portability::confirm(&app, &cookie, &review_b, false)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let cancelled = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("{review_b}/cancel"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cancelled.status(), StatusCode::SEE_OTHER);
    assert_eq!(cancelled.headers()[header::LOCATION].to_str().unwrap(), "/");
    assert_eq!(
        bears_db::list_bears_for_user(&pool, actor)
            .await
            .unwrap()
            .len(),
        1
    );
}
