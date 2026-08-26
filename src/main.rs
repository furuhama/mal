//! REPL / ファイル実行 (SPEC §10)。

mod core;
mod env;
mod eval;
mod printer;
mod reader;
mod types;

use std::io::Write;
use std::rc::Rc;

use crate::env::Env;
use crate::types::Value;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let env = core::default_env();
    match args.len() {
        1 => repl(&env),
        2 => run_file(&env, &args[1]),
        _ => {
            eprintln!("usage: mal [file.mal]");
            std::process::exit(2);
        }
    }
}

fn repl(env: &Rc<Env>) {
    let stdin = std::io::stdin();
    let mut buffer = String::new();
    let mut line = String::new();
    loop {
        if buffer.is_empty() {
            print!("mal=> ");
            std::io::stdout().flush().unwrap();
        }
        line.clear();
        match stdin.read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {
                buffer.push_str(&line);
                match reader::read_forms(&buffer) {
                    Ok(forms) => {
                        buffer.clear();
                        for form in forms {
                            if is_exit(&form) {
                                return;
                            }
                            match eval::eval_top(env, &form) {
                                Ok(v) => println!("{}", printer::pr_str(&v)),
                                Err(e) => println!("Error: {}", e),
                            }
                        }
                    }
                    Err(e) if e.eof => {
                        // 入力が途中で終わっている → 続きの行を待つ
                    }
                    Err(e) => {
                        println!("Error: {}", e);
                        buffer.clear();
                    }
                }
            }
            Err(e) => {
                eprintln!("読み取りエラー: {}", e);
                break;
            }
        }
    }
    println!();
}

fn is_exit(form: &Value) -> bool {
    matches!(form, Value::List(l) if l.len() == 1 && matches!(&l[0], Value::Symbol(s) if s == "exit"))
}

fn run_file(env: &Rc<Env>, path: &str) {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ファイルを開けません: {}", e);
            std::process::exit(1);
        }
    };
    let forms = match reader::read_forms(&src) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };
    for form in &forms {
        if let Err(e) = eval::eval_top(env, form) {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }
}
