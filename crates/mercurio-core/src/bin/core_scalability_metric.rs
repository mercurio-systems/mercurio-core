use std::error::Error;

use mercurio_core::{CoreScalabilityMetricConfig, run_core_scalability_metric};

fn main() -> Result<(), Box<dyn Error>> {
    let config = parse_args()?;
    let report = run_core_scalability_metric(config)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn parse_args() -> Result<CoreScalabilityMetricConfig, Box<dyn Error>> {
    let mut config = CoreScalabilityMetricConfig::default();
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--sizes" => {
                index += 1;
                let raw = args
                    .get(index)
                    .ok_or("--sizes requires a comma-separated value")?;
                config.model_sizes = raw
                    .split(',')
                    .filter(|value| !value.trim().is_empty())
                    .map(|value| value.trim().parse::<usize>())
                    .collect::<Result<Vec<_>, _>>()?;
            }
            "--edits" => {
                index += 1;
                let raw = args.get(index).ok_or("--edits requires a value")?;
                config.edit_count = raw.parse()?;
            }
            "--file" => {
                index += 1;
                config.target_file = args.get(index).ok_or("--file requires a value")?.clone();
            }
            "--package" => {
                index += 1;
                config.package_name = args.get(index).ok_or("--package requires a value")?.clone();
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            unknown => return Err(format!("unknown argument `{unknown}`").into()),
        }
        index += 1;
    }

    if config.model_sizes.is_empty() {
        return Err("--sizes must include at least one model size".into());
    }

    Ok(config)
}

fn print_help() {
    println!(
        "Usage: cargo run -p mercurio-core --bin core_scalability_metric -- [--sizes 100,1000,10000] [--edits 100] [--file scalability.sysml] [--package Scalability]"
    );
}
