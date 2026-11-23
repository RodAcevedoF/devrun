fn max_distance(input: &str) -> usize {
    (input.chars().count() / 3).max(1)
}

pub(crate) fn closest<'a>(
    input: &str,
    candidates: impl Iterator<Item = &'a str>,
) -> Option<String> {
    candidates
        .map(|candidate| (strsim::damerau_levenshtein(input, candidate), candidate))
        .filter(|(distance, _)| *distance <= max_distance(input))
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, candidate)| candidate.to_owned())
}
