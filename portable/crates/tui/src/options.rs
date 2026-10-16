use std::path::PathBuf;
#[derive(Debug, PartialEq)]
pub struct Options {
    pub endpoint: String,
    pub credential: PathBuf,
    pub name: String,
    pub ca: Option<PathBuf>,
    pub origin: Option<String>,
}
impl Options {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut endpoint = None;
        let mut credential = None;
        let mut name = None;
        let mut ca = None;
        let mut origin = None;
        if args.first().is_some_and(|s| !s.starts_with('-')) {
            if !(3..=5).contains(&args.len()) {
                return Err(
                    "Expected endpoint, credential file, approved name, optional CA and Origin."
                        .into(),
                );
            }
            endpoint = Some(args[0].clone());
            credential = Some(PathBuf::from(&args[1]));
            name = Some(args[2].clone());
            ca = args.get(3).map(PathBuf::from);
            origin = args.get(4).cloned();
        } else {
            let mut i = 0;
            while i < args.len() {
                let flag = &args[i];
                let value = args
                    .get(i + 1)
                    .filter(|v| !v.starts_with("--"))
                    .ok_or_else(|| format!("Missing value for {flag}."))?;
                match flag.as_str() {
                    "--server" if endpoint.is_none() => endpoint = Some(value.clone()),
                    "--credential" if credential.is_none() => {
                        credential = Some(PathBuf::from(value))
                    }
                    "--name" if name.is_none() => name = Some(value.clone()),
                    "--ca" if ca.is_none() => ca = Some(PathBuf::from(value)),
                    "--origin" if origin.is_none() => origin = Some(value.clone()),
                    _ => return Err(format!("Unknown or repeated option {flag}.")),
                }
                i += 2;
            }
        }
        let name = name.ok_or("--name must exactly match the approved grant name.")?;
        if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
            return Err(
                "Approved name must be 1–128 UTF-8 bytes with no control characters.".into(),
            );
        }
        Ok(Self {
            endpoint: endpoint.ok_or("--server is required.")?,
            credential: credential.ok_or("--credential is required.")?,
            name,
            ca,
            origin,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }
    #[test]
    fn exact_approved_name_survives_both_forms() {
        let flags = Options::parse(&args(&[
            "--server",
            "wss://host/control",
            "--credential",
            "client.json",
            "--name",
            "My Studio",
        ]))
        .unwrap();
        let old =
            Options::parse(&args(&["wss://host/control", "client.json", "My Studio"])).unwrap();
        assert_eq!(flags, old);
        assert_eq!(flags.name, "My Studio");
    }
    #[test]
    fn missing_repeated_and_control_names_reject() {
        for input in [
            vec!["--server", "wss://host/control"],
            vec!["--name", "one", "--name", "two"],
            vec!["wss://host/control", "client.json", "one\ntwo"],
        ] {
            assert!(Options::parse(&args(&input)).is_err());
        }
    }
}
