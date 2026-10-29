//! 需要联网的手工验证：确认 weapi 加密与二维码接口在真实服务端可用。
//!
//! 运行：`cargo test -p nm-api --test live -- --ignored --nocapture`

use nm_api::{ApiError, NeteaseClient, QrStatus, QrVerify};

#[tokio::test]
#[ignore = "需要访问 music.163.com"]
async fn public_playlist_detail() {
    // 云音乐热歌榜，公开歌单。
    let client = NeteaseClient::new();
    let detail = client.playlist_detail(3778678).await.expect("获取歌单详情");

    assert!(!detail.name.is_empty());
    assert!(!detail.tracks.is_empty(), "歌单没有返回歌曲");

    println!(
        "歌单：{} / {} 首（接口返回 {} 首），作者 {}",
        detail.name,
        detail.track_count,
        detail.tracks.len(),
        detail.creator_name()
    );
}

#[tokio::test]
#[ignore = "需要访问 music.163.com"]
async fn qr_login_roundtrip() {
    let client = NeteaseClient::new();

    let login = client.qr_key().await.expect("申请二维码 key");
    assert!(!login.key.is_empty());
    assert_eq!(
        login.url,
        format!("https://music.163.com/login?codekey={}", login.key)
    );

    let status = client.qr_check(&login.key).await.expect("轮询二维码状态");
    assert_eq!(status, QrStatus::Waiting);

    println!(
        "qr url ok, key length = {}, status = {:?}",
        login.key.len(),
        status
    );
}

#[tokio::test]
#[ignore = "需要访问 music.163.com"]
async fn verify_qr_endpoint_is_reachable() {
    let client = NeteaseClient::new();
    let challenge = QrVerify {
        verify_id: "invalid".to_string(),
        verify_type: serde_json::json!(2),
        verify_token: "invalid".to_string(),
        event_id: Some("invalid".to_string()),
        sign: Some("invalid".to_string()),
    };

    match client.verify_qr(&challenge).await {
        Ok(qr) => assert!(!qr.url.is_empty()),
        // 假参数应当被服务端以业务码拒绝，而不是 404 或解密失败。
        Err(ApiError::Api { code, message }) => {
            println!("verify/getqrcode -> {code}: {message}");
        }
        Err(other) => panic!("verify/getqrcode 返回了预期外的错误：{other:?}"),
    }

    match client.verify_status("invalid").await {
        Ok(status) => println!("verify/qrcodestatus -> {status:?}"),
        Err(ApiError::Api { code, message }) => {
            println!("verify/qrcodestatus -> {code}: {message}");
        }
        Err(ApiError::Decode(message)) => {
            println!("verify/qrcodestatus decode: {message}");
        }
        Err(other) => panic!("verify/qrcodestatus 返回了预期外的错误：{other:?}"),
    }
}

#[tokio::test]
#[ignore = "需要访问 music.163.com"]
async fn invalid_cookie_is_not_a_session() {
    let client = NeteaseClient::new();

    let mut cookies = std::collections::BTreeMap::new();
    cookies.insert("MUSIC_U".to_string(), "definitely-not-a-token".to_string());
    client.set_cookies(cookies);

    let profile = client.account().await.expect("account request");
    assert!(profile.is_none());
}
