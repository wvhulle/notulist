use crate::config::TextCleanupConfig;
use regex::Regex;
use std::collections::HashMap;

pub struct TextProcessor {
    spoken_punctuation: bool,
    replacements: HashMap<String, String>,
    remove_filler_words: bool,
    filler_re: Option<Regex>,
    filler_space_re: Regex,
    filler_punct_re: Regex,
    filler_dup_punct_re: Regex,
    filler_connector_before_term_re: Regex,
}

fn filler_collisions(language: Option<&str>) -> &'static [&'static str] {
    match language.map(|l| l.split(['-', '_']).next().unwrap_or(l)) {
        Some("pt") => &["um"],
        Some("de") => &["um"],
        _ => &[],
    }
}

impl TextProcessor {
    pub fn new(config: &TextCleanupConfig) -> Self {
        Self::new_for_language(config, None)
    }

    pub fn new_for_language(config: &TextCleanupConfig, language: Option<&str>) -> Self {
        let replacements = config
            .word_replacements
            .iter()
            .map(|(k, v)| (k.to_lowercase(), v.clone()))
            .collect();

        let effective_fillers: Vec<String> =
            if config.filler_words == crate::config::default_filler_words() {
                let collisions = filler_collisions(language);
                config
                    .filler_words
                    .iter()
                    .filter(|w| !collisions.contains(&w.to_lowercase().as_str()))
                    .cloned()
                    .collect()
            } else {
                config.filler_words.clone()
            };

        let filler_re = if config.remove_filler_words && !effective_fillers.is_empty() {
            let alternation = effective_fillers
                .iter()
                .filter(|w| !w.trim().is_empty())
                .map(|w| regex::escape(w.trim()))
                .collect::<Vec<_>>()
                .join("|");
            if alternation.is_empty() {
                None
            } else {
                let pattern = format!(r"(?i)\b(?:{})\b", alternation);
                Regex::new(&pattern).ok()
            }
        } else {
            None
        };

        let filler_space_re = Regex::new(r" {2,}")
            .expect("BUG: whitespace regex is a compile-time constant and must be valid");
        let filler_punct_re = Regex::new(r" +([,.;:!?])")
            .expect("BUG: punctuation regex is a compile-time constant and must be valid");
        let filler_dup_punct_re = Regex::new(r"([,;:])(\s*[,;:])+").expect(
            "BUG: duplicate-punctuation regex is a compile-time constant and must be valid",
        );
        let filler_connector_before_term_re = Regex::new(r"[,;:]+(\s*)([.!?])").expect(
            "BUG: connector-before-terminator regex is a compile-time constant and must be valid",
        );

        Self {
            spoken_punctuation: config.spoken_punctuation,
            replacements,
            remove_filler_words: config.remove_filler_words,
            filler_re,
            filler_space_re,
            filler_punct_re,
            filler_dup_punct_re,
            filler_connector_before_term_re,
        }
    }

    pub fn process(&self, text: &str) -> String {
        let mut result = text.to_string();

        if self.remove_filler_words {
            result = self.apply_filler_filter(&result);
        }

        if !self.replacements.is_empty() {
            result = self.apply_replacements(&result);
        }

        if self.spoken_punctuation {
            result = self.apply_spoken_punctuation(&result);
        }

        if !self.replacements.is_empty() {
            result = self.apply_replacements(&result);
        }

        result
    }

    fn apply_spoken_punctuation(&self, text: &str) -> String {
        let punctuation_map: &[(&str, &str)] = &[
            ("question mark", "?"),
            ("exclamation mark", "!"),
            ("exclamation point", "!"),
            ("open parenthesis", "("),
            ("close parenthesis", ")"),
            ("open paren", "("),
            ("close paren", ")"),
            ("open bracket", "["),
            ("close bracket", "]"),
            ("open brace", "{"),
            ("close brace", "}"),
            ("at sign", "@"),
            ("at symbol", "@"),
            ("dollar sign", "$"),
            ("percent sign", "%"),
            ("plus sign", "+"),
            ("equals sign", "="),
            ("forward slash", "/"),
            ("single quote", "'"),
            ("double quote", "\""),
            ("new paragraph", "\n\n"),
            ("new line", "\n"),
            ("period", "."),
            ("comma", ","),
            ("colon", ":"),
            ("semicolon", ";"),
            ("dash", "-"),
            ("hyphen", "-"),
            ("underscore", "_"),
            ("hash", "#"),
            ("hashtag", "#"),
            ("percent", "%"),
            ("ampersand", "&"),
            ("asterisk", "*"),
            ("plus", "+"),
            ("equals", "="),
            ("slash", "/"),
            ("backslash", "\\"),
            ("pipe", "|"),
            ("tilde", "~"),
            ("backtick", "`"),
            ("tab", "\t"),
        ];

        let converted = punctuation_map
            .iter()
            .fold(text.to_string(), |acc, (phrase, symbol)| {
                replace_phrase_case_insensitive(&acc, phrase, symbol)
            });
        clean_punctuation_spacing(&converted)
    }

    fn apply_filler_filter(&self, text: &str) -> String {
        let Some(re) = &self.filler_re else {
            return text.to_string();
        };

        let mut result = re.replace_all(text, " ").into_owned();

        result = self.filler_punct_re.replace_all(&result, "$1").into_owned();

        result = self
            .filler_dup_punct_re
            .replace_all(&result, "$1")
            .into_owned();

        result = self
            .filler_connector_before_term_re
            .replace_all(&result, "$2")
            .into_owned();

        result = self.filler_space_re.replace_all(&result, " ").into_owned();

        result
            .trim()
            .trim_start_matches([',', ';', ':'])
            .trim_start()
            .trim_end_matches([',', ';', ':'])
            .to_string()
    }

    fn apply_replacements(&self, text: &str) -> String {
        self.replacements
            .iter()
            .fold(text.to_string(), |acc, (word, replacement)| {
                replace_phrase_case_insensitive(&acc, word, replacement)
            })
    }
}

fn replace_phrase_case_insensitive(text: &str, from: &str, to: &str) -> String {
    let escaped = regex::escape(from);

    let pattern = format!(r"(?i)\b{}\b", escaped);

    match Regex::new(&pattern) {
        Ok(re) => re.replace_all(text, to).into_owned(),
        Err(_) => text.to_string(),
    }
}

fn drop_space_before(text: String, marks: &[char]) -> String {
    marks.iter().fold(text, |acc, mark| {
        acc.replace(&format!(" {mark}"), &mark.to_string())
    })
}

fn drop_space_after(text: String, marks: &[char]) -> String {
    marks.iter().fold(text, |acc, mark| {
        acc.replace(&format!("{mark} "), &mark.to_string())
    })
}

fn clean_punctuation_spacing(text: &str) -> String {
    let text = drop_space_before(
        text.to_string(),
        &['.', ',', '?', '!', ':', ';', ')', ']', '}'],
    );
    let text = drop_space_after(text, &['(', '[', '{']);
    let text = drop_space_before(text, &['(', '[', '{']);
    let text = drop_space_before(text, &['#', '@', '$']);
    let text = drop_space_after(text, &['#', '@', '$']);

    text.replace(" \n", "\n")
        .replace("\n ", "\n")
        .replace(" \t", "\t")
        .replace("\t ", "\t")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(spoken_punctuation: bool, replacements: &[(&str, &str)]) -> TextCleanupConfig {
        TextCleanupConfig {
            spoken_punctuation,
            word_replacements: replacements
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn portuguese_keeps_um() {
        let config = TextCleanupConfig {
            remove_filler_words: true,
            ..Default::default()
        };
        let pt = TextProcessor::new_for_language(&config, Some("pt"));
        assert_eq!(pt.process("Faça um commit"), "Faça um commit");

        let de = TextProcessor::new_for_language(&config, Some("de"));
        assert_eq!(de.process("Gehen wir um die Ecke"), "Gehen wir um die Ecke");
    }

    #[test]
    fn english_still_drops_um() {
        let config = TextCleanupConfig {
            remove_filler_words: true,
            ..Default::default()
        };
        let en = TextProcessor::new_for_language(&config, Some("en"));
        assert_eq!(en.process("so um let us commit"), "so let us commit");

        let unknown = TextProcessor::new_for_language(&config, None);
        assert_eq!(unknown.process("so um let us commit"), "so let us commit");
    }

    #[test]
    fn explicit_filler_list_is_not_narrowed() {
        let config = TextCleanupConfig {
            remove_filler_words: true,
            filler_words: vec!["um".to_string()],
            ..Default::default()
        };
        let pt = TextProcessor::new_for_language(&config, Some("pt"));
        assert_eq!(pt.process("Faça um commit"), "Faça commit");
    }

    #[test]
    fn regional_variants_resolve_to_base_language() {
        assert_eq!(filler_collisions(Some("pt-BR")), &["um"]);
        assert_eq!(filler_collisions(Some("pt_PT")), &["um"]);
        assert!(filler_collisions(Some("en-US")).is_empty());
    }

    #[test]
    fn batch_pipeline_output_is_pinned() {
        let config = make_config(true, &[("vox type", "voxtype")]);
        let processor = TextProcessor::new(&config);

        assert_eq!(
            processor.process("hello comma I use vox type for dictation period"),
            "hello, I use voxtype for dictation."
        );
    }

    #[test]
    fn test_spoken_punctuation_basic() {
        let config = make_config(true, &[]);
        let processor = TextProcessor::new(&config);

        assert_eq!(processor.process("hello period"), "hello.");
        assert_eq!(processor.process("hello comma world"), "hello, world");
        assert_eq!(processor.process("what question mark"), "what?");
    }

    #[test]
    fn test_spoken_punctuation_multi_word() {
        let config = make_config(true, &[]);
        let processor = TextProcessor::new(&config);

        assert_eq!(processor.process("open paren test close paren"), "(test)");
        assert_eq!(processor.process("hello exclamation mark"), "hello!");
    }

    #[test]
    fn test_spoken_punctuation_case_insensitive() {
        let config = make_config(true, &[]);
        let processor = TextProcessor::new(&config);

        assert_eq!(processor.process("hello PERIOD"), "hello.");
        assert_eq!(processor.process("hello Period"), "hello.");
    }

    #[test]
    fn test_word_replacements() {
        let config = make_config(false, &[("vox type", "voxtype")]);
        let processor = TextProcessor::new(&config);

        assert_eq!(
            processor.process("I use vox type for dictation"),
            "I use voxtype for dictation"
        );
    }

    #[test]
    fn test_word_replacements_case_insensitive() {
        let config = make_config(false, &[("rust", "Rust")]);
        let processor = TextProcessor::new(&config);

        assert_eq!(processor.process("I love RUST"), "I love Rust");
        assert_eq!(processor.process("rust is great"), "Rust is great");
    }

    #[test]
    fn test_disabled_processing() {
        let config = make_config(false, &[]);
        let processor = TextProcessor::new(&config);

        assert_eq!(processor.process("hello period"), "hello period");
    }

    #[test]
    fn test_combined_processing() {
        let config = make_config(true, &[("voxtype", "Voxtype")]);
        let processor = TextProcessor::new(&config);

        assert_eq!(processor.process("I use voxtype period"), "I use Voxtype.");
    }

    #[test]
    fn test_developer_punctuation() {
        let config = make_config(true, &[]);
        let processor = TextProcessor::new(&config);

        assert_eq!(
            processor.process("function open paren close paren"),
            "function()"
        );
        assert_eq!(
            processor.process("array open bracket close bracket"),
            "array[]"
        );
        assert_eq!(processor.process("hash include"), "#include");
        assert_eq!(processor.process("user at sign example"), "user@example");
    }

    #[test]
    fn test_newline_and_tab() {
        let config = make_config(true, &[]);
        let processor = TextProcessor::new(&config);

        assert_eq!(
            processor.process("line one new line line two"),
            "line one\nline two"
        );
        assert_eq!(processor.process("col one tab col two"), "col one\tcol two");
    }

    #[test]
    fn test_replacements_match_spoken_words_before_punctuation() {
        let config = make_config(true, &[("slash pr", "/pr")]);
        let processor = TextProcessor::new(&config);

        assert_eq!(processor.process("slash pr"), "/pr");
    }

    #[test]
    fn test_replacements_with_multiple_spoken_punctuation_words() {
        let config = make_config(true, &[("dash dash", "--")]);
        let processor = TextProcessor::new(&config);

        assert_eq!(processor.process("dash dash"), "--");
    }

    fn make_filler_config(enabled: bool, words: Option<Vec<&str>>) -> TextCleanupConfig {
        let filler_words = match words {
            Some(words) => words.into_iter().map(String::from).collect(),
            None => TextCleanupConfig::default().filler_words,
        };
        TextCleanupConfig {
            remove_filler_words: enabled,
            filler_words,
            ..Default::default()
        }
    }

    #[test]
    fn test_filler_filter_enabled_by_default() {
        let config = TextCleanupConfig::default();
        assert!(config.remove_filler_words);

        let processor = TextProcessor::new(&config);
        assert_eq!(processor.process("um hello"), "hello");
    }

    #[test]
    fn test_filler_filter_default_list() {
        let config = TextCleanupConfig::default();
        assert_eq!(
            config.filler_words,
            vec!["uh", "um", "er", "ah", "eh", "hmm", "hm", "mm", "mhm"]
        );
    }

    #[test]
    fn test_filler_filter_enabled_basic() {
        let config = make_filler_config(true, None);
        let processor = TextProcessor::new(&config);

        assert_eq!(processor.process("um hello world"), "hello world");
        assert_eq!(processor.process("hello uh world"), "hello world");
        assert_eq!(processor.process("hello world um"), "hello world");
    }

    #[test]
    fn test_filler_filter_case_insensitive() {
        let config = make_filler_config(true, None);
        let processor = TextProcessor::new(&config);

        assert_eq!(processor.process("UM hello"), "hello");
        assert_eq!(processor.process("Um hello"), "hello");
        assert_eq!(processor.process("Hmm I see"), "I see");
    }

    #[test]
    fn test_filler_filter_respects_word_boundaries() {
        let config = make_filler_config(true, None);
        let processor = TextProcessor::new(&config);

        assert_eq!(processor.process("umbrella"), "umbrella");
        assert_eq!(processor.process("an umbrella"), "an umbrella");
        assert_eq!(processor.process("summer"), "summer");
        assert_eq!(processor.process("hummingbird"), "hummingbird");
        assert_eq!(processor.process("erase the file"), "erase the file");
    }

    #[test]
    fn test_filler_filter_punctuation_cleanup_mid_sentence() {
        let config = make_filler_config(true, None);
        let processor = TextProcessor::new(&config);

        assert_eq!(processor.process("Well, um, I think"), "Well, I think");
    }

    #[test]
    fn test_filler_filter_punctuation_cleanup_start() {
        let config = make_filler_config(true, None);
        let processor = TextProcessor::new(&config);

        assert_eq!(processor.process("um, hello world"), "hello world");
        assert_eq!(processor.process("uh hello world"), "hello world");
    }

    #[test]
    fn test_filler_filter_punctuation_cleanup_end() {
        let config = make_filler_config(true, None);
        let processor = TextProcessor::new(&config);

        assert_eq!(processor.process("hello world, um"), "hello world");
        assert_eq!(processor.process("hello world, uh."), "hello world.");
    }

    #[test]
    fn test_filler_filter_back_to_back_fillers() {
        let config = make_filler_config(true, None);
        let processor = TextProcessor::new(&config);

        assert_eq!(processor.process("um uh hello"), "hello");
        assert_eq!(processor.process("hello um, uh, world"), "hello, world");
        assert_eq!(processor.process("um, uh, well"), "well");
    }

    #[test]
    fn test_filler_filter_preserves_sentence_punctuation() {
        let config = make_filler_config(true, None);
        let processor = TextProcessor::new(&config);

        assert_eq!(processor.process("hello um."), "hello.");
        assert_eq!(processor.process("hello um!"), "hello!");
        assert_eq!(processor.process("hello um?"), "hello?");
    }

    #[test]
    fn test_filler_filter_custom_list() {
        let config = make_filler_config(true, Some(vec!["like", "you know"]));
        let processor = TextProcessor::new(&config);

        assert_eq!(processor.process("um like hello"), "um hello");
        assert_eq!(processor.process("hello you know world"), "hello world");
    }

    #[test]
    fn test_filler_filter_empty_list_is_noop() {
        let config = make_filler_config(true, Some(vec![]));
        let processor = TextProcessor::new(&config);

        assert_eq!(processor.process("um hello"), "um hello");
    }

    #[test]
    fn test_filler_filter_runs_before_replacements() {
        let mut config = make_filler_config(true, None);
        config
            .word_replacements
            .insert("hello".to_string(), "HELLO".to_string());
        let processor = TextProcessor::new(&config);

        assert_eq!(processor.process("um hello uh world"), "HELLO world");
    }

    #[test]
    fn test_filler_filter_with_spoken_punctuation() {
        let mut config = make_filler_config(true, None);
        config.spoken_punctuation = true;
        let processor = TextProcessor::new(&config);

        assert_eq!(processor.process("well um I think period"), "well I think.");
    }
}
