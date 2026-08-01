use std::env;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorChoice {
    enabled: bool,
}

impl ColorChoice {
    pub fn new(no_color: bool) -> Self {
        Self {
            enabled: !no_color && env::var_os("NO_COLOR").is_none(),
        }
    }

    pub fn enabled(self) -> bool {
        self.enabled
    }

    pub fn paint(self, code: &str, text: impl AsRef<str>) -> String {
        let text = text.as_ref();
        if self.enabled {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    pub fn bold(self, text: impl AsRef<str>) -> String {
        self.paint("1", text)
    }

    pub fn dim(self, text: impl AsRef<str>) -> String {
        self.paint("2", text)
    }

    pub fn red(self, text: impl AsRef<str>) -> String {
        self.paint("31", text)
    }

    pub fn bold_green(self, text: impl AsRef<str>) -> String {
        self.paint("1;32", text)
    }

    pub fn bold_cyan(self, text: impl AsRef<str>) -> String {
        self.paint("1;36", text)
    }

    pub fn blue(self, text: impl AsRef<str>) -> String {
        self.paint("34", text)
    }

    pub fn green(self, text: impl AsRef<str>) -> String {
        self.paint("32", text)
    }

    pub fn yellow(self, text: impl AsRef<str>) -> String {
        self.paint("33", text)
    }

    pub fn magenta(self, text: impl AsRef<str>) -> String {
        self.paint("35", text)
    }
}
