#[tokio::test]
async fn test_health_endpoint() {
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };

    let handle = tokio::spawn(async move {
        aura_proxy::run_server(port).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let resp = reqwest::get(format!("http://127.0.0.1:{port}/health"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");

    handle.abort();
}
