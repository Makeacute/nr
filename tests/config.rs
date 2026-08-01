mod support;

use std::fs;

use nr::cli::InitConfigArgs;
use nr::config::{
    CheckSettings, ConfigInput, FlakeTarget, HookSettings, StateSettings, UiSettings, load_config,
    run_init_config, split_flake_reference, validate_flake_path,
};

#[test]
fn split_flake_reference_rejects_empty_fragment() {
    assert_eq!(
        split_flake_reference("/flake#host").unwrap(),
        ("/flake".to_string(), Some("host".to_string()))
    );
    assert!(split_flake_reference("/flake#").is_err());
}

#[test]
fn load_config_precedence_and_repo_overrides() {
    let temp = support::TestDir::new();
    let root = temp.path();
    let cli_flake = support::make_flake(&root.join("cli"));
    let env_flake = support::make_flake(&root.join("env"));
    let nearest = support::make_flake(&root.join("nearest"));
    let user_flake = support::make_flake(&root.join("user"));
    let nested = nearest.join("nested");
    fs::create_dir_all(&nested).unwrap();

    let xdg = root.join("xdg");
    fs::create_dir_all(xdg.join("nr")).unwrap();
    fs::write(
        xdg.join("nr/config.toml"),
        format!(
            r##"
[target]
flake = "{}"
host = "user-host"

[check]
nixfmt = true
commands = [
  ["echo", "[ok]"],
  ["sh", "-c", "printf '[done]'"],
]

[publish]
remote = "upstream"

[hooks]
post_switch = [["systemctl", "--user", "restart", "waybar.service"]]

[ui]
accent = "#cba6f7"
"##,
            user_flake.display()
        ),
    )
    .unwrap();

    let config = load_config(ConfigInput {
        flake: Some(format!("{}#fragment-host", cli_flake.display())),
        host: Some("cli-host".to_string()),
        cwd: Some(nested.clone()),
        environ: Some(vec![
            ("XDG_CONFIG_HOME".to_string(), xdg.display().to_string()),
            (
                "NR_FLAKE".to_string(),
                format!("{}#env-fragment", env_flake.display()),
            ),
            ("NR_HOST".to_string(), "env-host".to_string()),
        ]),
        hostname: Some("machine".to_string()),
    })
    .unwrap();
    assert_eq!(
        config.target,
        FlakeTarget {
            path: cli_flake,
            host: "cli-host".to_string()
        }
    );

    fs::write(
        nearest.join(".nr.toml"),
        r#"
[target]
host = "repo-host"

[check]
statix = true
"#,
    )
    .unwrap();
    let config = load_config(ConfigInput {
        cwd: Some(nested),
        environ: Some(vec![
            ("XDG_CONFIG_HOME".to_string(), xdg.display().to_string()),
            ("NR_HOST".to_string(), "env-host".to_string()),
        ]),
        ..ConfigInput::default()
    })
    .unwrap();
    assert_eq!(config.target.host, "env-host");
    assert_eq!(
        config.check,
        CheckSettings {
            nixfmt: true,
            statix: true,
            commands: vec![
                vec!["echo".to_string(), "[ok]".to_string()],
                vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    "printf '[done]'".to_string(),
                ],
            ],
            ..CheckSettings::default()
        }
    );
    assert_eq!(config.publish.remote, "upstream");
    assert_eq!(
        config.hooks,
        HookSettings {
            post_switch: vec![vec![
                "systemctl".to_string(),
                "--user".to_string(),
                "restart".to_string(),
                "waybar.service".to_string(),
            ]],
            ..HookSettings::default()
        }
    );
    assert_eq!(
        config.ui,
        UiSettings {
            accent: Some("#cba6f7".to_string()),
            ..UiSettings::default()
        }
    );
    assert_eq!(config.state, StateSettings::default());
}

#[test]
fn validate_flake_path_requires_flake_file() {
    let temp = support::TestDir::new();
    assert!(validate_flake_path(temp.path()).is_err());
    support::make_flake(temp.path());
    validate_flake_path(temp.path()).unwrap();
}

#[test]
fn init_config_honors_explicit_flake_path() {
    let temp = support::TestDir::new();
    let cwd_flake = support::make_flake(&temp.path().join("cwd"));
    let selected_flake = support::make_flake(&temp.path().join("selected"));

    run_init_config(
        &ConfigInput {
            flake: Some(format!("{}#host", selected_flake.display())),
            cwd: Some(cwd_flake.clone()),
            environ: Some(vec![]),
            ..ConfigInput::default()
        },
        &InitConfigArgs {
            user: false,
            force: false,
        },
    )
    .unwrap();

    assert!(selected_flake.join(".nr.toml").is_file());
    assert!(!cwd_flake.join(".nr.toml").exists());
}

#[test]
fn config_rejects_unknown_keys() {
    let temp = support::TestDir::new();
    let flake = support::make_flake(&temp.path().join("flake"));
    let xdg = temp.path().join("xdg");
    fs::create_dir_all(xdg.join("nr")).unwrap();
    fs::write(
        xdg.join("nr/config.toml"),
        format!(
            r#"
[target]
flake = "{}"

[check]
typo = true
"#,
            flake.display()
        ),
    )
    .unwrap();

    let error = load_config(ConfigInput {
        cwd: Some(temp.path().to_path_buf()),
        environ: Some(vec![(
            "XDG_CONFIG_HOME".to_string(),
            xdg.display().to_string(),
        )]),
        ..ConfigInput::default()
    })
    .unwrap_err()
    .to_string();

    assert!(error.contains("unknown field"));
}

#[test]
fn config_rejects_invalid_ui_accent() {
    let temp = support::TestDir::new();
    let flake = support::make_flake(&temp.path().join("flake"));
    let xdg = temp.path().join("xdg");
    fs::create_dir_all(xdg.join("nr")).unwrap();
    fs::write(
        xdg.join("nr/config.toml"),
        format!(
            r##"
[target]
flake = "{}"

[ui]
accent = "purple"
"##,
            flake.display()
        ),
    )
    .unwrap();

    let error = load_config(ConfigInput {
        cwd: Some(temp.path().to_path_buf()),
        environ: Some(vec![(
            "XDG_CONFIG_HOME".to_string(),
            xdg.display().to_string(),
        )]),
        ..ConfigInput::default()
    })
    .unwrap_err()
    .to_string();

    assert!(error.contains("[ui].accent"));
}
