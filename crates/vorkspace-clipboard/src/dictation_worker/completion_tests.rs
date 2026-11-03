use super::suggest;

#[test]
fn reported_examples_and_prefix_boundaries() {
    for text in [
        "What is your name",
        "That sounds great. What is your name",
        "Hi there, what is your name",
        "Can you take a look at this",
        "That sounds great. Hi there, what is your name  ",
    ] {
        assert_eq!(suggest(text), Some('?'), "{text:?}");
    }
    for text in [
        "What is important matters",
        "What is said matters",
        "That sounds great. What is important matters",
        "I want to go home",
        "I want to go home.",
        "What is your name?  ",
        "What is your name!",
        "Hi there, what is your name.",
        "I wonder, what is your name",
        "Please ask Dr. Smith. What is your name",
        "The estimate is approx. What is your name",
        "Visit example.com. What is your name",
        "Use git status. What is your name",
        "What is your user_id",
        "¿Cómo está usted",
        "What is your café",
    ] {
        assert_eq!(suggest(text), None, "{text:?}");
    }
}

// Hand-labelled design challenges. An abstention on a question is a missed
// mark; a mark on any other group is an incorrect change of meaning.
#[test]
fn direct_questions_and_non_questions() {
    let groups: &[(&str, bool, &[&str])] = &[
        (
            "direct questions",
            true,
            &[
                "Where did you leave my keys",
                "Can you take a look at this",
                "Are you coming with us",
                "What is the address",
                "How are you feeling today",
                "Why did she leave early",
                "Will they arrive before lunch",
                "Is the server running",
                "What did you say",
                "Where is my jacket",
                "Who is that",
                "Do you know where the file is",
                "What time does the meeting start",
                "What is your favorite color",
                "Where is the nearest hospital",
                "Could you please help me",
                "Is it ready",
                "May I borrow your umbrella",
                "What can we do today",
                "Can Alex help us tomorrow",
                "Did she send the report",
                "When are we leaving",
                "Would you like some tea",
                "What is your plan",
                "Can you tell me where the keys are",
            ],
        ),
        (
            "free relatives and statements",
            false,
            &[
                "What is important matters",
                "What is said matters",
                "What is done matters",
                "What is true varies",
                "What is mine belongs to me",
                "What is missing is a key",
                "What is available depends on timing",
                "What is the best way depends on you",
                "What is your plan remains unclear",
                "Where they live is private",
                "Where we meet matters",
                "How it works is complicated",
                "How she arrived remains unknown",
                "Should you need assistance call me",
                "Had you stayed we could meet",
                "Were she here she would agree",
                "Can you help is the question",
                "I want to go home",
                "I finished the report yesterday",
                "I hope you are having a wonderful day",
            ],
        ),
        (
            "indirect questions",
            false,
            &[
                "I wonder what your name is",
                "I wonder, what is your name",
                "Tell me where you live",
                "I need to know how this works",
                "The question is whether you can come",
                "Please tell me what is your name",
                "She asked, what is your name",
                "I am not sure where it is",
                "The answer depends on what is available",
                "I wonder whether you are ready",
            ],
        ),
        (
            "fragments",
            false,
            &[
                "What is your",
                "What is the",
                "Where do you",
                "Can you",
                "Can you take",
                "Can you please take",
                "Can you look at",
                "Is the old chair",
                "What is your favorite",
                "Could you look at",
                "What is the very",
                "How can we get to",
                "Where did you place the",
                "What is your plan for",
                "Can you help me with",
                "How are you going to",
                "Is this the right",
                "Do you want the new",
                "What is the best",
                "What did you say about",
            ],
        ),
        (
            "abbreviations and technical text",
            false,
            &[
                "Dr. Smith is here. What is your name",
                "We saw Prof. Lee. Where are you now",
                "The meeting is at 5 p.m. What is your name",
                "The estimate is approx. What is your name",
                "Please open example.com. What is your name",
                "What is https://example.com",
                "What is your user_id",
                "What is your userID",
                "Can you run git status",
                "Can you run cargo build",
                "Is the API ready now",
                "What is your URL",
                "What is the value 42",
                "Are there twenty guests",
                "Can you open config.json",
                "Can you use snake_case",
                "Can you select the variable name",
                "What is your email@example.com",
                "Can you open the file/path",
                "Can you run npm install",
                "Can you use camel case",
                "Can you write snake case",
                "Can you open kubectl",
                "Can you wait until eleventh",
                "Can you split this into halves",
                "Can you wait until half",
            ],
        ),
    ];
    for &(name, is_question, cases) in groups {
        let (mut marks, mut abstentions, mut wrong) = (0, 0, 0);
        for &text in cases {
            match (is_question, suggest(text)) {
                (true, Some('?')) => marks += 1,
                (_, None) => abstentions += 1,
                (_, Some(mark)) => {
                    wrong += 1;
                    println!("wrong {name}: {text:?} -> {mark:?}");
                }
            }
        }
        println!(
            "{name}: total={} correct_marks={marks} wrong_marks={wrong} abstentions={abstentions}",
            cases.len()
        );
        assert_eq!(wrong, 0, "{name}");
    }
}

// A separate hand-labelled set of new phrasings, including forms outside the
// selected grammar. It is not a random sample and is not recognizer truth.
#[test]
fn held_out_phrasings() {
    let groups: &[(&str, bool, &[&str])] = &[
        (
            "held-out direct questions",
            true,
            &[
                "What is your phone",
                "Where are your shoes",
                "Why did you leave the room",
                "Who is your doctor",
                "How is your family",
                "Can we go to the park",
                "Is the kettle boiling",
                "Could you pass that",
                "Can you check on this",
                "Do they want to join us",
                "Would you give me a hand",
                "Why are you looking at me",
                "When did it happen",
                "What does it mean",
                "Where is the elevator",
                "What's your name",
                "What happened to the parcel",
                "Which route is shorter",
                "Is the water too hot",
                "Hey there, can you help me",
                "Everything looks fine. Can you check on this",
                "Thanks, how are you feeling",
                "I see, can you look at this",
                "Do you have time",
            ],
        ),
        (
            "held-out free relatives and statements",
            false,
            &[
                "What is needed should be provided",
                "What is available changes daily",
                "What is yours is safe",
                "What is unknown remains unknown",
                "What is the hardest part varies",
                "What is the best solution depends on context",
                "Where you go depends on weather",
                "How they arrived remains a mystery",
                "Whoever calls gets a reply",
                "That is what is happening",
                "Should you arrive early wait outside",
                "Had they known things would be different",
                "Were we ready nothing would change",
                "I believe this is a good idea",
                "We are meeting tomorrow",
                "You can hear me",
            ],
        ),
        (
            "held-out indirect questions",
            false,
            &[
                "I wonder where my parcel is",
                "She knows what is needed",
                "I asked, where is your bag",
                "I wonder, where is my coat",
                "I need to know what is true",
                "Tell me how you found it",
                "The question is what we should do",
                "Please explain why they left",
                "She asked if you can come",
                "He is unsure where they went",
            ],
        ),
        (
            "held-out fragments",
            false,
            &[
                "What is my",
                "Could you please send",
                "Can you bring the",
                "Why are they going to",
                "Did she put the",
                "What is your current",
                "What is the wrong",
                "Do you want the red",
                "Can you please give",
                "Can you take a look at",
                "Can you let me know if",
                "What is your opinion about",
                "Where did you put the",
                "What do you think about",
                "Where can we go to",
                "Is the old table",
                "Can you please make",
                "What is your preferred",
            ],
        ),
        (
            "held-out abbreviations and technical text",
            false,
            &[
                "I met Mr. Parker. Is the door open",
                "The meeting is at 8 a.m. Can you help",
                "She wrote e.g. Can you look at this",
                "We spoke to the Rev. What is your name",
                "Send it via config.ini. What is your name",
                "Hi there, how is the API",
                "Can you run git diff",
                "Is userID valid now",
                "What is the value 123",
                "Is the URL correct",
                "Visit https://example.org. What is your name",
                "Can you use user_id",
                "What is the json format",
                "Is localhost working",
                "Can you open path/to/file",
                "Do you have nine tickets",
                "Is the function ready",
                "Use npm install. Can you check on this",
            ],
        ),
    ];
    for &(name, is_question, cases) in groups {
        let (mut marks, mut abstentions, mut wrong) = (0, 0, 0);
        for &text in cases {
            match (is_question, suggest(text)) {
                (true, Some('?')) => marks += 1,
                (_, None) => abstentions += 1,
                (_, Some(mark)) => {
                    wrong += 1;
                    println!("wrong {name}: {text:?} -> {mark:?}");
                }
            }
        }
        println!(
            "{name}: total={} correct_marks={marks} wrong_marks={wrong} abstentions={abstentions}",
            cases.len()
        );
        assert_eq!(wrong, 0, "{name}");
    }
}
