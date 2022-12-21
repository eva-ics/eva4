use clap::{Parser, Subcommand};
use colored::Colorize;
use eva_common::prelude::*;
use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;

#[derive(Subcommand)]
enum Command {
    #[clap(about = "generate .po files")]
    Generate,
    #[clap(about = "compile .po files to .mo")]
    Compile,
    #[clap(about = "display stats (translated/missing)")]
    Stat,
}

const HEADER: &str = r#"#, fuzzy
msgid ""
msgstr ""
"Content-Type: text/plain; charset=utf-8\n"
"#;

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Command::Generate => "generate",
                Command::Compile => "compile",
                Command::Stat => "stat",
            }
        )
    }
}

#[derive(Parser)]
#[clap(author, version, about, long_about = None)]
struct Opts {
    #[clap(
        name = "FILE",
        help = "File to parse (YAML or JSON), MUST be in UI directory"
    )]
    fname: String,
    #[clap(subcommand)]
    command: Command,
    #[clap(
        long = "lang",
        short = 'l',
        required = true,
        help = "language, required, can be specified multiple times"
    )]
    lang: Vec<String>,
    #[clap(
        long = "ui-dir",
        short = 'u',
        required = true,
        help = "UI directory root, required"
    )]
    ui_dir: String,
    #[clap(
        long = "output-dir",
        short = 'o',
        required = true,
        help = "Output directory (usually EVA_DIR/pvt/locales), required"
    )]
    output_dir: String,
}

fn parse_content(value: Value, ids: &mut Vec<String>) {
    // do not use hash as the order need to be kept
    if let Value::String(mut s) = value {
        s.retain(|c| c != '\r');
        for chunk in s.split('\n') {
            let id = chunk.trim();
            if !id.is_empty() {
                let m = id.to_owned();
                if !ids.contains(&m) {
                    ids.push(m);
                }
            }
        }
    } else if let Value::Seq(s) = value {
        for c in s {
            parse_content(c, ids);
        }
    } else if let Value::Map(s) = value {
        for (_, c) in s {
            parse_content(c, ids);
        }
    }
}

#[allow(clippy::similar_names)]
#[allow(clippy::too_many_lines)]
fn main() -> EResult<()> {
    let opts = Opts::parse();
    let fpath = Path::new(&opts.fname);
    let ui_dir = Path::new(&opts.ui_dir).canonicalize()?;
    let output_dir = Path::new(&opts.output_dir).canonicalize()?;
    let can_path = fpath.canonicalize()?;
    let mut relpath = can_path
        .strip_prefix(&ui_dir)
        .map_err(|_| {
            Error::failed(format!(
                "The file {} is not in {} directory",
                opts.fname, opts.ui_dir
            ))
        })?
        .to_path_buf();
    relpath.pop();
    let data = fs::read(&opts.fname)?;
    let content: Value = serde_yaml::from_slice(&data).map_err(Error::invalid_data)?;
    let mut msg_ids: Vec<String> = Vec::new();
    parse_content(content, &mut msg_ids);
    for lang in opts.lang {
        let mut po_buf = PathBuf::from(&output_dir);
        po_buf.push(lang);
        po_buf.push("LC_MESSAGES");
        po_buf.extend(&relpath);
        fs::create_dir_all(&po_buf)?;
        po_buf.push(format!(
            "{}.po",
            fpath.file_stem().unwrap().to_str().unwrap()
        ));
        let po_fname = Path::new(&po_buf);
        print!(
            " [{}] {} ",
            opts.command.to_string().to_uppercase().green().bold(),
            po_fname.to_str().unwrap()
        );
        io::stdout().flush()?;
        match opts.command {
            Command::Generate => {
                let mut lang_msg_ids = msg_ids.clone();
                let write_header = if po_fname.exists() {
                    let mut existing: HashSet<&str> = HashSet::new();
                    let data = fs::read_to_string(po_fname)?;
                    for line in data.split('\n') {
                        if let Some(message) = line.strip_prefix("msgid ") {
                            let mut msgid = message.trim();
                            if !msgid.starts_with('"') || !msgid.ends_with('"') || msgid.len() < 2 {
                                return Err(Error::invalid_data(format!(
                                    "Invalid msgid {}",
                                    msgid
                                )));
                            }
                            msgid = &msgid[1..msgid.len() - 1];
                            if !msgid.is_empty() {
                                existing.insert(msgid);
                            }
                        }
                    }
                    lang_msg_ids.retain(|m| !existing.contains(m.as_str()));
                    false
                } else {
                    true
                };
                if !lang_msg_ids.is_empty() || write_header {
                    let mut po = fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(po_fname)?;
                    if write_header {
                        po.write_all(HEADER.as_bytes())?;
                    }
                    for m in &lang_msg_ids {
                        let line = format!("\nmsgid \"{m}\"\nmsgstr \"\"\n");
                        po.write_all(line.as_bytes())?;
                    }
                }
                let s = format!("+{}", lang_msg_ids.len());
                println!(
                    "{}",
                    if lang_msg_ids.is_empty() {
                        s.dimmed()
                    } else {
                        s.cyan()
                    }
                );
            }
            Command::Compile => {
                let mo_fname = po_fname.with_extension("mo");
                let need_compile = if mo_fname.exists() {
                    let mo_metadata = fs::metadata(&mo_fname)?;
                    let po_metadata = fs::metadata(po_fname)?;
                    mo_metadata.modified()? < po_metadata.modified()?
                } else {
                    true
                };
                if need_compile {
                    let status = process::Command::new("msgfmt")
                        .args([po_fname.to_str().unwrap(), "-o", mo_fname.to_str().unwrap()])
                        .status()
                        .map_err(|e| Error::failed(format!("Unable to execute msgfmt: {}", e)))?;
                    if status.success() {
                        println!("{}", "OK".green().bold());
                    } else {
                        println!("{}", "FAILED".red().bold());
                    }
                } else {
                    println!("{}", "skipped".dimmed());
                }
            }
            Command::Stat => {
                let data = fs::read_to_string(po_fname)?;
                let mut total = 0;
                let mut translated = 0;
                for line in data.split('\n') {
                    if let Some(message) = line.strip_prefix("msgstr ") {
                        let mut msgstr = message.trim();
                        if !msgstr.starts_with('"') || !msgstr.ends_with('"') || msgstr.len() < 2 {
                            return Err(Error::invalid_data(format!("Invalid msgstr {}", msgstr)));
                        }
                        msgstr = &msgstr[1..msgstr.len() - 1];
                        total += 1;
                        if !msgstr.is_empty() {
                            translated += 1;
                        }
                    }
                }
                print!(" translated: {}", translated);
                let missing = total - translated - 1;
                if missing > 0 {
                    print!(", missing: {}", missing.to_string().yellow());
                }
                println!();
            }
        }
    }
    Ok(())
}
