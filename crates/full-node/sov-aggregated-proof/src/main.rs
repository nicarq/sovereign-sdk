use hex::FromHex;
use risc0_zkp::core::digest::Digest;
use sov_aggregated_proof::check_receipts;

fn main() {
    let method_id =
        Digest::from_hex("665839999d6b39fff2bfce839e709d4eb0eb75cdcda76219729fb81b5fd381ca")
            .unwrap();

    check_receipts(
        vec![
            "data/inner_0.proof",
            "data/inner_1.proof",
            "data/inner_2.proof",
            "data/inner_3.proof",
            "data/inner_4.proof",
            "data/inner_5.proof",
        ],
        method_id,
    )
    .unwrap();
}
