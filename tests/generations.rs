mod support;

use nr::generations::{
    current_generation, generation_by_number, pin_generation, previous_generation,
    read_pins_from_path, resolve_generation_reference,
};

#[test]
fn pins_generation_labels_and_requires_force_to_overwrite() {
    let temp = support::TestDir::new();
    let path = temp.path().join("pins.toml");

    pin_generation(42, "last-good", false, &path).unwrap();
    let pins = read_pins_from_path(&path).unwrap();
    assert_eq!(pins.pins.get("last-good"), Some(&42));
    assert_eq!(
        resolve_generation_reference("last-good", &pins).unwrap(),
        42
    );
    assert_eq!(resolve_generation_reference("43", &pins).unwrap(), 43);

    assert!(pin_generation(41, "last-good", false, &path).is_err());
    pin_generation(41, "last-good", true, &path).unwrap();
    let pins = read_pins_from_path(&path).unwrap();
    assert_eq!(pins.pins.get("last-good"), Some(&41));
}

#[test]
fn parses_generation_json_and_finds_previous_current() {
    let generations = nr::generations::parse_generations_json(
        r#"
[
  {"generation":2,"date":"2026-08-01 10:00:00","nixosVersion":"26.11","kernelVersion":"6.18.40","configurationRevision":"rev2","specialisations":[],"current":true},
  {"generation":1,"date":"2026-07-31 10:00:00","nixosVersion":"26.11","kernelVersion":"6.18.39","configurationRevision":"rev1","specialisations":[],"current":false}
]
"#,
    )
    .unwrap();

    assert_eq!(current_generation(&generations).unwrap().generation, 2);
    assert_eq!(previous_generation(&generations).unwrap().generation, 1);
    assert_eq!(
        generation_by_number(&generations, 1).unwrap().date,
        "2026-07-31 10:00:00"
    );
}
