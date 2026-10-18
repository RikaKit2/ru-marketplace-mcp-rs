use ru_marketplace_core::transport::send_with_transient_retry;
use std::time::Duration;

#[tokio::test]
async fn test_retry_on_fake_connect_error() {
    let mut attempts = 0;
    let res = send_with_transient_retry(2, Duration::from_millis(10), || {
        attempts += 1;
        async {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_millis(50))
                .build()
                .unwrap();
            client.get("http://127.0.0.1:9").send().await
        }
    })
    .await;

    assert!(res.is_err());
    assert_eq!(attempts, 3);
}
