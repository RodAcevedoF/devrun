use crate::process::ExitCode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrepareOutcome {
    Succeeded,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StepOutcome {
    Next { step: usize },
    Ended(PrepareOutcome),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum RerunRequest {
    #[default]
    NotRequested,
    Requested,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrepareEnd {
    RerunFromFirstStep,
    Report(PrepareOutcome),
}

impl RerunRequest {
    pub fn rerun_or_report(self, outcome: PrepareOutcome) -> PrepareEnd {
        match self {
            Self::Requested => PrepareEnd::RerunFromFirstStep,
            Self::NotRequested => PrepareEnd::Report(outcome),
        }
    }
}

pub(crate) fn step_outcome(step: usize, total: usize, code: ExitCode) -> StepOutcome {
    if code != Some(0) {
        StepOutcome::Ended(PrepareOutcome::Failed(format!(
            "{} {}",
            step_label(step, total),
            failure_description(code)
        )))
    } else if step + 1 < total {
        StepOutcome::Next { step: step + 1 }
    } else {
        StepOutcome::Ended(PrepareOutcome::Succeeded)
    }
}

pub(crate) fn could_not_start(step: usize, total: usize, reason: &str) -> String {
    format!("{} could not start: {reason}", step_label(step, total))
}

fn step_label(step: usize, total: usize) -> String {
    format!("prepare step {}/{total}", step + 1)
}

fn failure_description(code: ExitCode) -> String {
    match code {
        Some(code) => format!("failed with code {code}"),
        None => "was terminated by a signal".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_successful_step_advances_until_the_last_one_ends_the_prepare() {
        assert_eq!(step_outcome(0, 2, Some(0)), StepOutcome::Next { step: 1 });
        assert_eq!(
            step_outcome(1, 2, Some(0)),
            StepOutcome::Ended(PrepareOutcome::Succeeded)
        );
    }

    #[test]
    fn a_failing_step_ends_the_prepare_naming_the_step_and_the_failure() {
        assert_eq!(
            step_outcome(0, 2, Some(4)),
            StepOutcome::Ended(PrepareOutcome::Failed(
                "prepare step 1/2 failed with code 4".to_owned()
            ))
        );
        assert_eq!(
            step_outcome(1, 2, None),
            StepOutcome::Ended(PrepareOutcome::Failed(
                "prepare step 2/2 was terminated by a signal".to_owned()
            ))
        );
        assert_eq!(
            could_not_start(0, 3, "no such file"),
            "prepare step 1/3 could not start: no such file"
        );
    }

    #[test]
    fn a_requested_rerun_supersedes_both_outcomes() {
        let failed = PrepareOutcome::Failed("prepare step 1/1 failed with code 2".to_owned());
        for outcome in [PrepareOutcome::Succeeded, failed.clone()] {
            assert_eq!(
                RerunRequest::Requested.rerun_or_report(outcome),
                PrepareEnd::RerunFromFirstStep
            );
        }
        assert_eq!(
            RerunRequest::NotRequested.rerun_or_report(failed.clone()),
            PrepareEnd::Report(failed)
        );
    }
}
