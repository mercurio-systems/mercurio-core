use std::env;
use std::path::PathBuf;

use mercurio_core::frontend::sysml::parse_sysml;

const DEFAULT_PILOT_ROOT: &str = "../SysML-v2-Pilot-Implementation";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut pilot_root = PathBuf::from(DEFAULT_PILOT_ROOT);
    let mut paths_file = None;
    let args = env::args().skip(1).collect::<Vec<_>>();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--pilot-root" => {
                index += 1;
                pilot_root = PathBuf::from(args.get(index).ok_or("missing --pilot-root value")?);
            }
            "--paths-file" => {
                index += 1;
                paths_file = Some(PathBuf::from(
                    args.get(index).ok_or("missing --paths-file value")?,
                ));
            }
            _ => return Err(format!("unknown argument `{}`", args[index]).into()),
        }
        index += 1;
    }
    let paths_file = paths_file.ok_or("usage: parse_sysml_paths --paths-file <path>")?;
    let mut pass_count = 0usize;
    let mut fail_count = 0usize;

    for relative_path in std::fs::read_to_string(paths_file)?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let path = pilot_root.join(relative_path.replace('/', std::path::MAIN_SEPARATOR_STR));
        let text = std::fs::read_to_string(&path)?;
        match parse_sysml(&text) {
            Ok(_) => {
                pass_count += 1;
                println!("PASS {relative_path}");
            }
            Err(err) => {
                fail_count += 1;
                println!("FAIL {relative_path}: {}", err.message);
                if let Some(span) = err.span {
                    println!("  at {}:{}", span.start_line, span.start_col);
                }
            }
        }
    }

    println!("summary: pass={pass_count} fail={fail_count}");
    Ok(())
}
