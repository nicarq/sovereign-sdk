use sov_metrics::{init_metrics_tracker, MonitoringConfig};
use sov_modules_api::{Gas, GasMeter};
use sov_modules_macros::track_gas_constants_usage;
use tokio::net::UdpSocket;
use tokio::time::timeout;

type S = sov_test_utils::TestSpec;

use sov_modules_api::{GasSpec, Spec};

#[track_gas_constants_usage(input)]
fn test_metrics(input: &mut u64) {
    let mut meter = sov_modules_api::BasicGasMeter::<S>::new_with_gas(
        <S as Spec>::Gas::from([100, 100]),
        S::initial_base_fee_per_gas(),
    );

    *input *= 10;

    let constant = <S as Spec>::Gas::from([1, 1]).with_name("test".to_string());

    meter.charge_gas(&constant).unwrap();
}

/// Test that the gas constant is correctly tracked using the `track_gas_constants_usage` macro.
#[tokio::test(flavor = "multi_thread")]
async fn test_metrics_macro() {
    // Setting up an udp channel to listen from
    let channel = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("Impossible to bind to port");

    // Insert metadata for the metrics
    sov_metrics::METRICS_METADATA
        .write()
        .unwrap()
        .insert("test_metadata".to_string(), "test_value".to_string());

    init_metrics_tracker(&MonitoringConfig {
        telegraf_address: channel.local_addr().unwrap(),
        max_datagram_size: Some(1),
        max_pending_metrics: None,
    });

    let input = &mut 10;

    // Check that no gas constant is tracked yet
    assert!(sov_metrics::GAS_CONSTANTS.with(|gas_constants| gas_constants.borrow().is_empty()));

    test_metrics(input);

    // We have one invocation of the metric here.
    let mut buf = [0; 1024];
    timeout(
        std::time::Duration::from_secs(1),
        channel.recv_from(&mut buf),
    )
    .await
    .expect("Timeout while waiting for the UDP channel to receive data")
    .unwrap();

    let mut parsed_buf = std::str::from_utf8(&buf[..]).unwrap().split(" ");
    assert_eq!(
        parsed_buf.next().unwrap(),
        "sov_rollup_gas_constant,name=test_metrics,constant=test,input=10,test_metadata=test_value"
    );
    assert_eq!(parsed_buf.next().unwrap(), "num_invocations=1");

    // The gas constant map should be updated
    assert_eq!(
        sov_metrics::GAS_CONSTANTS
            .with(|gas_constants| *gas_constants.borrow().get("test").unwrap()),
        1
    );

    test_metrics(input);

    // We should have only one invocation here as well.
    let mut buf = [0; 1024];
    timeout(
        std::time::Duration::from_secs(1),
        channel.recv_from(&mut buf),
    )
    .await
    .expect("Timeout while waiting for the UDP channel to receive data")
    .unwrap();

    let mut parsed_buf = std::str::from_utf8(&buf[..]).unwrap().split(" ");
    assert_eq!(
        parsed_buf.next().unwrap(),
        "sov_rollup_gas_constant,name=test_metrics,constant=test,input=100,test_metadata=test_value"
    );
    assert_eq!(parsed_buf.next().unwrap(), "num_invocations=1");

    // The gas constants map is updated
    assert_eq!(
        sov_metrics::GAS_CONSTANTS
            .with(|gas_constants| *gas_constants.borrow().get("test").unwrap()),
        2
    );
}

#[track_gas_constants_usage]
fn test_metrics_without_input() {
    let mut meter = sov_modules_api::BasicGasMeter::<S>::new_with_gas(
        <S as Spec>::Gas::from([100, 100]),
        S::initial_base_fee_per_gas(),
    );

    let constant = <S as Spec>::Gas::from([1, 1]).with_name("test".to_string());

    meter.charge_gas(&constant).unwrap();
}

/// Test that the gas constant is correctly tracked using the `track_gas_constants_usage` macro.
#[tokio::test(flavor = "multi_thread")]
async fn test_metrics_macro_without_input() {
    // Setting up an udp channel to listen from
    let channel = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("Impossible to bind to port");

    init_metrics_tracker(&MonitoringConfig {
        telegraf_address: channel.local_addr().unwrap(),
        max_datagram_size: Some(1),
        max_pending_metrics: None,
    });

    // Check that no gas constant is tracked yet
    assert!(sov_metrics::GAS_CONSTANTS.with(|gas_constants| gas_constants.borrow().is_empty()));

    test_metrics_without_input();

    // We have one invocation of the metric here.
    let mut buf = [0; 1024];
    timeout(
        std::time::Duration::from_secs(1),
        channel.recv_from(&mut buf),
    )
    .await
    .expect("Timeout while waiting for the UDP channel to receive data")
    .unwrap();

    let mut parsed_buf = std::str::from_utf8(&buf[..]).unwrap().split(" ");
    assert_eq!(
        parsed_buf.next().unwrap(),
        "sov_rollup_gas_constant,name=test_metrics_without_input,constant=test"
    );
    assert_eq!(parsed_buf.next().unwrap(), "num_invocations=1");

    // The gas constant map should be updated
    assert_eq!(
        sov_metrics::GAS_CONSTANTS
            .with(|gas_constants| *gas_constants.borrow().get("test").unwrap()),
        1
    );
}
