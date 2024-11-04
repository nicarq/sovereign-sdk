use rest_api_load_testing::Requests;

#[tokio::main]
async fn main() {
    let requests = Requests::new("http://localhost:12346", vec!["ledger/slots/latest"]);
    let report = rest_api_load_testing::start(requests).await;
    println!("{report:?}");
}
