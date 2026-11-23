use std::io::{self, IsTerminal};
use std::process;

use cliclack::ProgressBar;

use super::prompter::{CANCELLED, Choice, Prompter, Validator};

const MAX_ROWS: usize = 15;

pub fn is_interactive() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

pub fn or_exit_cancelled<T>(result: io::Result<T>) -> anyhow::Result<T> {
    match result {
        Err(error) if error.kind() == io::ErrorKind::Interrupted => exit_cancelled(),
        other => Ok(other?),
    }
}

pub fn exit_cancelled() -> ! {
    let _ = cliclack::outro_cancel(CANCELLED);
    exit_failure()
}

pub fn exit_failure() -> ! {
    process::exit(1)
}

#[derive(Default)]
pub struct ClackPrompter {
    spinner: Option<ProgressBar>,
}

impl Prompter for ClackPrompter {
    fn intro(&mut self, title: &str) -> io::Result<()> {
        cliclack::intro(title)
    }

    fn start_spinner(&mut self, message: &str) {
        let spinner = cliclack::spinner();
        spinner.start(message);
        self.spinner = Some(spinner);
    }

    fn stop_spinner(&mut self, message: &str) {
        if let Some(spinner) = self.spinner.take() {
            spinner.stop(message);
        }
    }

    fn note(&mut self, title: &str, body: &str) -> io::Result<()> {
        cliclack::note(title, body)
    }

    fn warning(&mut self, message: &str) -> io::Result<()> {
        cliclack::log::warning(message)
    }

    fn error(&mut self, message: &str) -> io::Result<()> {
        cliclack::log::error(message)
    }

    fn multiselect(
        &mut self,
        prompt: &str,
        choices: &[Choice],
        required: bool,
    ) -> io::Result<Vec<usize>> {
        let items: Vec<(usize, &str, &str)> = choices
            .iter()
            .enumerate()
            .map(|(index, choice)| (index, choice.label.as_str(), choice.hint.as_str()))
            .collect();
        let checked: Vec<usize> = choices
            .iter()
            .enumerate()
            .filter(|(_, choice)| choice.checked)
            .map(|(index, _)| index)
            .collect();
        cliclack::multiselect(prompt)
            .items(&items)
            .initial_values(checked)
            .required(required)
            .max_rows(MAX_ROWS)
            .interact()
    }

    fn input(&mut self, prompt: &str, initial: &str, validate: Validator) -> io::Result<String> {
        let mut input = cliclack::input(prompt).validate(move |value: &String| validate(value));
        if !initial.is_empty() {
            input = input.default_input(initial);
        }
        input.interact()
    }

    fn confirm(&mut self, prompt: &str, initial: bool) -> io::Result<bool> {
        cliclack::confirm(prompt).initial_value(initial).interact()
    }

    fn outro(&mut self, message: &str) -> io::Result<()> {
        cliclack::outro(message)
    }

    fn cancelled(&mut self, message: &str) -> io::Result<()> {
        cliclack::outro_cancel(message)
    }
}
