use console::{Style, style};

pub struct Cell {
    text: String,
    style: Style,
}

impl Cell {
    pub fn new(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }

    pub fn plain(text: impl Into<String>) -> Self {
        Self::new(text, Style::new())
    }
}

pub fn table(rows: &[Vec<Cell>]) -> Vec<String> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|column| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(|cell| cell.text.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    rows.iter()
        .map(|row| {
            let last_filled = row.iter().rposition(|cell| !cell.text.is_empty());
            let cells = row
                .iter()
                .enumerate()
                .take(last_filled.map_or(0, |index| index + 1))
                .map(|(column, cell)| {
                    let padded = if Some(column) == last_filled {
                        cell.text.clone()
                    } else {
                        format!("{:<width$}", cell.text, width = widths[column])
                    };
                    cell.style.apply_to(padded).to_string()
                });
            format!("  {}", cells.collect::<Vec<_>>().join("  "))
        })
        .collect()
}

pub fn heading(text: &str) -> String {
    style(text).bold().to_string()
}

pub fn none() -> String {
    format!("  {}", style("(none)").dim())
}

pub fn name_style() -> Style {
    Style::new().cyan().bold()
}

pub fn dim_style() -> Style {
    Style::new().dim()
}

pub fn print_warning(message: &str) {
    eprintln!("{} {message}", style("warning:").yellow().bold());
}

pub fn print_error(error: &anyhow::Error) {
    eprintln!("{} {error:#}", style("error:").red().bold());
}
