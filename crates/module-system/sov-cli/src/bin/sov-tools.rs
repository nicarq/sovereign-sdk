use sov_modules_api::clap;
use sov_modules_api::clap::Parser;

#[derive(Parser)]
#[command(author, version, about = "Sovereign tools: helpers for privacy + zk-poc", long_about = None)]
struct App {
    #[command(subcommand)]
    workflow: Workflow,
}

#[derive(clap::Subcommand, Clone, Debug)]
enum Workflow {
    #[clap(subcommand)]
    Privacy(sov_cli::workflows::privacy::PrivacyWorkflow),
    #[clap(subcommand)]
    ZkPoc(sov_cli::workflows::zk_poc::ZkPocWorkflow),
}

fn main() -> anyhow::Result<()> {
    let app = App::parse();
    match app.workflow {
        Workflow::Privacy(w) => w.run(),
        Workflow::ZkPoc(w) => w.run(),
    }
}
