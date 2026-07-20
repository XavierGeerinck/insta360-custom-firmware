use std::path::PathBuf;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut args = std::env::args_os();
    let program = args.next().unwrap_or_else(|| "retention-acceptance".into());
    let Some(root) = args.next() else {
        eprintln!(
            "usage: {} /tmp/waved-retention-acceptance-<id>",
            PathBuf::from(program).display()
        );
        std::process::exit(2);
    };
    if args.next().is_some() {
        eprintln!("retention-acceptance accepts exactly one fixture root");
        std::process::exit(2);
    }

    match waved_meeting::retention::run_acceptance_fixture(&PathBuf::from(root)).await {
        Ok(report) => println!("{report}"),
        Err(error) => {
            eprintln!("retention acceptance failed: {error}");
            std::process::exit(1);
        }
    }
}
