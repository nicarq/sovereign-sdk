use rest_api_load_testing::Requests;

#[tokio::main]
async fn main() {
    let requests = Requests::new("http://localhost:12346", vec!["ledger/slots/latest"]);
    let reports = rest_api_load_testing::start(requests).await;

    for report in reports {
        for m in report.measurements {
            println!("{:?} {:?}", m.time, m.output);
        }
    }
}
