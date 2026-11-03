//! A deliberately narrow English text heuristic, used only on explicit Stop.
//! It has no access to prosody or intent and cannot identify every question.

pub(super) fn suggest(text: &str) -> Option<char> {
    let text = text.trim();
    // Refuse punctuation (including a terminal mark), identifiers, numbers,
    // multiple sentences and non-English text rather than trying to parse them.
    if text.len() > 512
        || !text
            .bytes()
            .all(|byte| byte.is_ascii_alphabetic() || byte == b' ')
    {
        return None;
    }
    let words: Vec<_> = text.split_ascii_whitespace().collect();
    if !(4..=24).contains(&words.len())
        || words.iter().any(|word| {
            (word.len() > 1 && !word[1..].bytes().all(|byte| byte.is_ascii_lowercase()))
                || excluded(&word.to_ascii_lowercase())
        })
    {
        return None;
    }
    let words: Vec<_> = words.iter().map(|word| word.to_ascii_lowercase()).collect();
    if dangling(words.last()?.as_str())
        || (words[words.len() - 2] == "that" && words.last()?.as_str() == "you")
        || words.windows(2).any(|pair| {
            matches!(pair[0].as_str(), "camel" | "snake" | "kebab") && pair[1] == "case"
        })
    {
        return None;
    }
    let word = |index: usize| words.get(index).map(String::as_str).unwrap_or("");
    let determiner = matches!(word(1), "the" | "a" | "your" | "my" | "our" | "their");
    let plausible_predicate = words[2..].iter().any(|word| {
        word.ends_with("ing")
            || word.ends_with("ed")
            || matches!(
                word.as_str(),
                "ready"
                    | "open"
                    | "closed"
                    | "available"
                    | "correct"
                    | "awake"
                    | "here"
                    | "there"
                    | "start"
                    | "begin"
                    | "arrive"
                    | "leave"
                    | "work"
                    | "meet"
            )
    });
    let inverted = auxiliary(word(0))
        && matches!(
            word(1),
            "i" | "you"
                | "we"
                | "he"
                | "she"
                | "they"
                | "it"
                | "this"
                | "that"
                | "the"
                | "a"
                | "your"
                | "my"
                | "our"
                | "their"
        )
        && (!determiner || plausible_predicate);
    let wh =
        matches!(word(0), "what" | "where" | "when" | "why" | "how" | "who") && auxiliary(word(1));
    // A second auxiliary may introduce a larger statement containing a
    // question-shaped clause. Refuse it even though it also excludes some
    // perfectly valid direct questions.
    if (inverted || wh) && !words[2..].iter().any(|word| auxiliary(word)) {
        return Some('?');
    }

    // First-person expressions of thought, desire, or expectation are useful
    // statement candidates; ordinary declaratives such as "you sent it" are
    // particularly likely to be echo questions. Even these candidates can be
    // spoken as questions, so the result is a best-effort guess, not intent.
    let thought = matches!(word(1), "think" | "believe" | "hope" | "expect" | "feel")
        && words.len() >= 5
        && words[2..].iter().any(|word| auxiliary(word));
    let desire = matches!(word(1), "want" | "need" | "plan")
        && word(2) == "to"
        && action(word(3))
        && words.len() >= 5;
    let would_like = word(1) == "would"
        && word(2) == "like"
        && word(3) == "to"
        && action(word(4))
        && words.len() >= 6;
    (word(0) == "i" && (thought || desire || would_like)).then_some('.')
}

fn action(word: &str) -> bool {
    matches!(
        word,
        "go" | "walk"
            | "leave"
            | "stay"
            | "sleep"
            | "rest"
            | "travel"
            | "meet"
            | "talk"
            | "know"
            | "read"
    )
}

fn auxiliary(word: &str) -> bool {
    matches!(
        word,
        "am" | "is"
            | "are"
            | "was"
            | "were"
            | "do"
            | "does"
            | "did"
            | "can"
            | "could"
            | "will"
            | "would"
            | "should"
            | "may"
            | "might"
            | "must"
            | "shall"
            | "has"
            | "have"
            | "had"
    )
}

fn dangling(word: &str) -> bool {
    word.ends_with("ly")
        || auxiliary(word)
        || matches!(
            word,
            "a" | "an"
                | "the"
                | "my"
                | "your"
                | "his"
                | "her"
                | "our"
                | "their"
                | "this"
                | "that"
                | "these"
                | "those"
                | "some"
                | "any"
                | "each"
                | "every"
                | "to"
                | "of"
                | "for"
                | "with"
                | "at"
                | "by"
                | "from"
                | "in"
                | "on"
                | "into"
                | "about"
                | "up"
                | "out"
                | "off"
                | "over"
                | "under"
                | "through"
                | "as"
                | "and"
                | "or"
                | "but"
                | "because"
                | "if"
                | "whether"
                | "i"
                | "he"
                | "she"
                | "we"
                | "they"
                | "it"
                | "what"
                | "which"
                | "who"
                | "whose"
                | "where"
                | "when"
                | "why"
                | "how"
                | "not"
                | "very"
                | "really"
                | "most"
                | "favorite"
                | "latest"
                | "newest"
                | "already"
                | "still"
                | "almost"
                | "quite"
                | "rather"
                | "too"
                | "so"
                | "supposed"
                | "be"
                | "been"
                | "being"
                | "get"
                | "become"
                | "seem"
                | "bring"
                | "send"
                | "give"
                | "tell"
                | "make"
                | "find"
                | "take"
                | "put"
                | "choose"
                | "pick"
                | "select"
                | "use"
                | "open"
                | "close"
                | "read"
                | "know"
                | "need"
                | "want"
                | "ask"
                | "show"
                | "say"
                | "explain"
        )
}

fn excluded(word: &str) -> bool {
    matches!(
        word,
        "zero"
            | "one"
            | "two"
            | "three"
            | "four"
            | "five"
            | "six"
            | "seven"
            | "eight"
            | "nine"
            | "ten"
            | "eleven"
            | "twelve"
            | "thirteen"
            | "fourteen"
            | "fifteen"
            | "sixteen"
            | "seventeen"
            | "eighteen"
            | "nineteen"
            | "twenty"
            | "thirty"
            | "forty"
            | "fifty"
            | "sixty"
            | "seventy"
            | "eighty"
            | "ninety"
            | "hundred"
            | "thousand"
            | "million"
            | "billion"
            | "trillion"
            | "half"
            | "quarter"
            | "dozen"
            | "first"
            | "second"
            | "third"
            | "fourth"
            | "fifth"
            | "sixth"
            | "seventh"
            | "eighth"
            | "ninth"
            | "tenth"
            | "eleventh"
            | "twelfth"
            | "thirteenth"
            | "fourteenth"
            | "fifteenth"
            | "sixteenth"
            | "seventeenth"
            | "eighteenth"
            | "nineteenth"
            | "twentieth"
            | "thirtieth"
            | "fortieth"
            | "fiftieth"
            | "sixtieth"
            | "seventieth"
            | "eightieth"
            | "ninetieth"
            | "hundredth"
            | "thousandth"
            | "millionth"
            | "billionth"
            | "trillionth"
            | "number"
            | "digit"
            | "digits"
            | "url"
            | "http"
            | "https"
            | "www"
            | "dot"
            | "slash"
            | "underscore"
            | "hyphen"
            | "colon"
            | "semicolon"
            | "equals"
            | "identifier"
            | "variable"
            | "function"
            | "command"
            | "terminal"
            | "code"
            | "api"
            | "json"
            | "sql"
            | "git"
            | "github"
            | "cargo"
            | "npm"
            | "python"
            | "javascript"
            | "rust"
            | "docker"
            | "kubectl"
            | "localhost"
    )
}

#[cfg(test)]
mod tests {
    use super::suggest;

    #[test]
    fn refuses_existing_marks_and_non_plain_text() {
        for text in [
            "What is your name?  ",
            "What is your name!",
            "I think it is ready.",
            "What is your name, again",
            "What is your_name",
            "What is the URL https://example.com",
            "What is userID here",
            "What is your name\nplease",
            "Comment allez vous aujourd'hui",
            "¿Cómo está usted",
        ] {
            assert_eq!(suggest(text), None, "{text:?}");
        }
        assert_eq!(suggest("What is your name  "), Some('?'));
        assert_eq!(suggest(&"a".repeat(513)), None);
    }

    // Hand-labelled examples, not recognizer output or a sample of user traffic.
    // The contrasting declarative pairs make the unavoidable intent ambiguity
    // visible. Several inputs deliberately fall outside the chosen shapes.
    #[test]
    fn representative_intent_and_abstention_examples() {
        let groups: &[(&str, Option<char>, &[&str], usize)] = &[
            (
                "wh questions",
                Some('?'),
                &[
                    "What is your name",
                    "Where is the train station",
                    "How does this work",
                    "Why did the build fail",
                    "When are you leaving tomorrow",
                    "Who is your favorite singer",
                    "What are we having for dinner",
                    "How can we help today",
                    "What happened yesterday",
                    "What time does the train leave",
                    "Who called you last night",
                    "Which color would you prefer",
                ],
                0,
            ),
            (
                "inverted yes/no questions",
                Some('?'),
                &[
                    "Is the server running",
                    "Did she enjoy the trip",
                    "Can we meet on Friday",
                    "Are the children still awake",
                    "Has your package arrived yet",
                    "Will the meeting start soon",
                    "Does he know your name",
                    "Would you like some tea",
                    "Should I leave now",
                    "May we borrow the chairs",
                    "Is it ready",
                    "Do the dogs bark",
                    "Can Alex come over",
                ],
                0,
            ),
            (
                "declarative questions",
                Some('?'),
                &[
                    "You sent the report",
                    "The meeting starts at noon",
                    "It is ready",
                    "You can hear me",
                    "I finished the report yesterday",
                    "The weather is very pleasant today",
                    "We are meeting tomorrow",
                    "Your name is Alex",
                    "I should send the file",
                    "I want to go home",
                    "I think we should leave now",
                    "I would like to know your name",
                ],
                3,
            ),
            (
                "statements",
                Some('.'),
                &[
                    "I think we should leave now",
                    "I hope you are feeling better",
                    "I believe we will arrive tomorrow",
                    "I expect the rain will stop",
                    "I feel the idea is useful",
                    "I want to go home",
                    "I would like to know your name",
                    "I plan to travel next summer",
                    "I need to rest for now",
                    "I want to meet the neighbors",
                    "I finished the report yesterday",
                    "The meeting begins at noon",
                    "We had dinner earlier",
                    "She walked home",
                    "The weather is nice today",
                    "I wish you good luck",
                    "I hope the weather improves soon",
                ],
                0,
            ),
            (
                "fragments",
                None,
                &[
                    "The meeting with",
                    "I think that",
                    "Are we going to",
                    "What I really wanted to say",
                    "Because the server",
                    "If you have time",
                    "I think the weather tomorrow",
                    "I want to send my colleague",
                    "What is your favorite",
                    "Is the old chair",
                    "I think we should",
                    "I think the weather will be",
                    "I think the report is being",
                    "I think the weather will get",
                    "What do you think about",
                    "I would like to know who",
                    "Can you tell me where",
                    "I want to talk to my",
                    "I want to go to the",
                    "Could you please bring",
                ],
                0,
            ),
            (
                "numbers",
                None,
                &[
                    "42",
                    "What is 42 plus 8",
                    "I think the price is 42",
                    "Do you have forty apples",
                    "What is your telephone number",
                    "I want to go at seven",
                    "I think there are twenty people",
                    "Can I have three tickets",
                    "The value is 42",
                    "I need two hundred dollars",
                ],
                0,
            ),
            (
                "technical input",
                None,
                &[
                    "Run cargo build",
                    "What is git status",
                    "I think the API is ready",
                    "Please open example.com",
                    "What is https://example.com",
                    "I think user_id is correct",
                    "What is userID here",
                    "I think we should use camel case",
                    "What is the environment variable",
                    "I want to read the JSON",
                    "Could you run npm install",
                    "What is the command for that",
                    "I think this code is ready",
                    "Please select user underscore ID",
                    "How does the function work",
                ],
                0,
            ),
        ];
        for &(category, intended, cases, max_wrong) in groups {
            let (mut correct, mut wrong, mut abstained) = (0, 0, 0);
            for &text in cases {
                match suggest(text) {
                    Some(actual) if Some(actual) == intended => correct += 1,
                    Some(actual) => {
                        wrong += 1;
                        println!("wrong {category}: {text:?} -> {actual:?}, intended {intended:?}");
                    }
                    None => abstained += 1,
                }
            }
            println!(
                "{category}: total={} correct={correct} wrong={wrong} abstained={abstained}",
                cases.len()
            );
            assert!(wrong <= max_wrong, "{category}: {wrong} incorrect marks");
        }
    }

    // A second hand-written set, chosen after the shapes above. It includes
    // grammatical forms and vocabulary the heuristic does not recognize.
    #[test]
    fn additional_phrasings() {
        let groups: &[(&str, Option<char>, &[&str], usize)] = &[
            (
                "wh questions",
                Some('?'),
                &[
                    "What is the nearest hospital",
                    "Where did they go for dinner",
                    "How will you get there",
                    "Why is everyone so quiet",
                    "Who wrote the letter",
                    "How many people attended",
                    "What are the ingredients",
                    "Which train leaves first",
                    "When can you come over",
                    "What does this button do",
                ],
                0,
            ),
            (
                "inverted yes/no questions",
                Some('?'),
                &[
                    "Do you like the music",
                    "Could she visit next week",
                    "Is the bread fresh",
                    "Did the train arrive late",
                    "Are they still waiting",
                    "Would you help me tomorrow",
                    "Is your appointment confirmed",
                    "Does it seem safe",
                    "Should we take the bus",
                    "Has the bus gone yet",
                ],
                0,
            ),
            (
                "declarative questions",
                Some('?'),
                &[
                    "I think this is the right house",
                    "I need to sleep right now",
                    "I would like to walk outside",
                    "I want to read the latest chapter",
                    "The dog is asleep",
                    "You have finished the work",
                    "We should go home",
                    "I finished early this morning",
                    "The package arrived yesterday",
                    "She is coming along",
                ],
                4,
            ),
            (
                "statements",
                Some('.'),
                &[
                    "I think this is the right house",
                    "I need to sleep right now",
                    "I would like to walk outside",
                    "I want to read the latest chapter",
                    "The bread smells wonderful",
                    "She is arriving tomorrow",
                    "I hope that you will be okay",
                    "I want to stay here tonight",
                    "I believe it is ready now",
                    "I intend to leave soon",
                    "I walked home after dinner",
                    "The package arrived yesterday",
                ],
                0,
            ),
            (
                "fragments",
                None,
                &[
                    "I think the next train will arrive at",
                    "Do you know how to",
                    "I would like to read the latest",
                    "I think the room is probably",
                    "I believe the meeting is still",
                    "Will the very old house",
                    "What is the answer to",
                    "I think the room is",
                    "I want to know which",
                    "Could you please bring the",
                    "I expect that you will be",
                    "Where are my",
                    "I think the report is almost",
                    "I think the project is supposed",
                    "What is it that you",
                ],
                0,
            ),
            (
                "numbers",
                None,
                &[
                    "Can you give me twelve apples",
                    "I think half the cake is ready",
                    "What is the first name",
                    "Do we need a dozen eggs",
                    "Is 2026 the year",
                    "I want to go at six",
                    "How much is 9 dollars",
                    "I think one is enough",
                    "What is your sixth choice",
                    "Is the ninth chapter ready",
                ],
                0,
            ),
            (
                "technical input",
                None,
                &[
                    "What is the file_path",
                    "I think python is available",
                    "Is github online now",
                    "Could you check the XML",
                    "I want to read the server.log",
                    "What does localhost mean here",
                    "I think the Docker command is ready",
                    "Is the function called parseValue",
                    "I want to go to example.com",
                    "I think the snake case is wrong",
                ],
                0,
            ),
        ];
        for &(category, intended, cases, max_wrong) in groups {
            let (mut correct, mut wrong, mut abstained) = (0, 0, 0);
            for &text in cases {
                match suggest(text) {
                    Some(actual) if Some(actual) == intended => correct += 1,
                    Some(actual) => {
                        wrong += 1;
                        println!(
                            "additional wrong {category}: {text:?} -> {actual:?}, intended {intended:?}"
                        );
                    }
                    None => abstained += 1,
                }
            }
            println!(
                "additional {category}: total={} correct={correct} wrong={wrong} abstained={abstained}",
                cases.len()
            );
            assert!(wrong <= max_wrong, "{category}: {wrong} incorrect marks");
        }
    }
}
