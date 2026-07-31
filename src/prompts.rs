use std::io::{self, Write};

pub fn confirm(prompt: &str, default: bool) -> bool {
    let suffix = if default { " [Y/n] " } else { " [y/N] " };
    loop {
        print!("{prompt}{suffix}");
        let _ = io::stdout().flush();
        let mut answer = String::new();
        match io::stdin().read_line(&mut answer) {
            Ok(0) => return false,
            Ok(_) => {
                let answer = answer.trim().to_lowercase();
                if answer.is_empty() {
                    return default;
                }
                if matches!(answer.as_str(), "y" | "yes") {
                    return true;
                }
                if matches!(answer.as_str(), "n" | "no") {
                    return false;
                }
                println!("Please answer yes or no.");
            }
            Err(_) => return false,
        }
    }
}

pub fn choose(prompt: &str, choices: &[(&str, &str)], default: Option<&str>) -> Option<String> {
    for (key, label) in choices {
        let marker = if Some(*key) == default {
            " (default)"
        } else {
            ""
        };
        println!("  {key}: {label}{marker}");
    }
    loop {
        let suffix = default
            .map(|value| format!(" [{value}] "))
            .unwrap_or_else(|| " ".to_string());
        print!("{prompt}{suffix}");
        let _ = io::stdout().flush();
        let mut answer = String::new();
        match io::stdin().read_line(&mut answer) {
            Ok(0) => return None,
            Ok(_) => {
                let answer = answer.trim();
                if answer.is_empty() {
                    return default.map(ToOwned::to_owned);
                }
                if choices.iter().any(|(key, _)| *key == answer) {
                    return Some(answer.to_string());
                }
                println!(
                    "Please choose one of: {}.",
                    choices
                        .iter()
                        .map(|(key, _)| *key)
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            Err(_) => return None,
        }
    }
}

pub fn read_line(prompt: &str, default: Option<&str>) -> Option<String> {
    let suffix = default
        .map(|value| format!(" [{value}] "))
        .unwrap_or_else(|| " ".to_string());
    print!("{prompt}{suffix}");
    let _ = io::stdout().flush();
    let mut answer = String::new();
    match io::stdin().read_line(&mut answer) {
        Ok(0) | Err(_) => None,
        Ok(_) => {
            let answer = answer.trim();
            if answer.is_empty() {
                default.map(ToOwned::to_owned)
            } else {
                Some(answer.to_string())
            }
        }
    }
}
