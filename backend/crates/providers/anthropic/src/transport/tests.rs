//! 隐私门禁同时覆盖推理和所有控制请求，失败前不得向本机模拟上游发包。

use super::*;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::any};

#[tokio::test]
async fn unavailable_veil_blocks_every_outbound_purpose_before_network_io() {
    let directory = tempfile::tempdir().unwrap();
    let veil = gateway_veil::Runtime::initialize(gateway_veil::Config {
        data_directory: directory.path().join("uninitialized"),
        initialize_store: false,
        ..Default::default()
    })
    .await;
    let transport = Transport::new(veil);
    assert!(transport.client(None).is_err());
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for purpose in [
        Purpose::Inference,
        Purpose::Control(gateway_veil::ControlPurpose::OAuth),
        Purpose::Control(gateway_veil::ControlPurpose::Profile),
        Purpose::Control(gateway_veil::ControlPurpose::Quota),
        Purpose::Control(gateway_veil::ControlPurpose::Models),
    ] {
        assert!(matches!(
            transport.send(client.get(server.uri()), purpose).await,
            Err(SendError::Privacy(_))
        ));
    }
    server.verify().await;
}
