#[tokio::main]
async fn main() {
    let client = sov_rollup_json_client::Client::new("http://example.com");

    let response = client.get_latest_base_fee_per_gas().await.unwrap();
    println!("Response: {:?}", response);
}
