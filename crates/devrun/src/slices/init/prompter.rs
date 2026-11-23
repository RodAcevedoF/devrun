use std::io;

pub const CANCELLED: &str = "Cancelled; nothing was saved.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub label: String,
    pub hint: String,
    pub checked: bool,
}

pub type Validator = fn(&str) -> Result<(), String>;

pub trait Prompter {
    fn intro(&mut self, title: &str) -> io::Result<()>;

    fn start_spinner(&mut self, message: &str);

    fn stop_spinner(&mut self, message: &str);

    fn note(&mut self, title: &str, body: &str) -> io::Result<()>;

    fn warning(&mut self, message: &str) -> io::Result<()>;

    fn error(&mut self, message: &str) -> io::Result<()>;

    fn multiselect(
        &mut self,
        prompt: &str,
        choices: &[Choice],
        required: bool,
    ) -> io::Result<Vec<usize>>;

    fn input(&mut self, prompt: &str, initial: &str, validate: Validator) -> io::Result<String>;

    fn confirm(&mut self, prompt: &str, initial: bool) -> io::Result<bool>;

    fn outro(&mut self, message: &str) -> io::Result<()>;

    fn cancelled(&mut self, message: &str) -> io::Result<()>;
}
