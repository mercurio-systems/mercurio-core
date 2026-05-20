use std::path::PathBuf;

use mercurio_core::frontend::ast::{Declaration, QualifiedName};
use mercurio_core::frontend::sysml::parse_sysml;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: inspect_sysml_parse <input.sysml>")?;
    let text = std::fs::read_to_string(PathBuf::from(path))?;
    let module = parse_sysml(&text)?;

    for member in &module.members {
        dump_decl(member, 0);
    }
    Ok(())
}

fn dump_decl(declaration: &Declaration, depth: usize) {
    let pad = "  ".repeat(depth);
    match declaration {
        Declaration::Package(package) => {
            println!("{pad}package {}", package.name.as_dot_string());
            for member in &package.members {
                dump_decl(member, depth + 1);
            }
        }
        Declaration::Import(import) => {
            println!("{pad}import {}", display_name(&import.path));
        }
        Declaration::PartDefinition(definition) => {
            println!("{pad}part def {}", definition.name);
            for member in &definition.members {
                dump_decl(member, depth + 1);
            }
        }
        Declaration::PartUsage(usage) => {
            println!("{pad}part {}{}", usage.name, display_type(usage.ty.as_ref()));
            for member in &usage.body_members {
                dump_decl(member, depth + 1);
            }
        }
        Declaration::GenericDefinition(definition) => {
            println!("{pad}{} def {}", definition.keyword, definition.name);
            for member in &definition.members {
                dump_decl(member, depth + 1);
            }
        }
        Declaration::GenericUsage(usage) => {
            println!(
                "{pad}{} {}{}",
                usage.keyword,
                usage.name,
                display_type(usage.ty.as_ref())
            );
            for member in &usage.body_members {
                dump_decl(member, depth + 1);
            }
        }
        Declaration::Alias(alias) => {
            println!("{pad}alias {} for {}", alias.name, display_name(&alias.target));
        }
    }
}

fn display_name(name: &QualifiedName) -> String {
    name.as_colon_string()
}

fn display_type(name: Option<&QualifiedName>) -> String {
    name.map(|name| format!(" : {}", name.as_colon_string()))
        .unwrap_or_default()
}
