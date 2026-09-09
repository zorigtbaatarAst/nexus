//! A symptom is written in plain words, and plain words used to seed nothing.
//!
//! `seeds::targets` accepted a word only if it carried a capital, an underscore, or a path
//! separator — a rule that suited "refactor PaymentService" and refused "the cache serves a
//! stale package". Four real defects fixed in this repository, given to the context engine as
//! their symptoms, produced zero hits and three empty packages, while the code they named sat
//! in the index the whole time.

use nexus_core::context::{Intent, Purpose, Seed, SeedStrength, TaskRequest, TASK_BUDGET_TOKENS};
use nexus_core::Engine;
use std::fs;
use std::path::{Path, PathBuf};

fn git(root: &Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .expect("git")
        .status
        .success();
    assert!(ok, "git {args:?} failed");
}

/// A fixture shaped like the code the symptoms were about: a lowercase module name that
/// occurs exactly once, another that occurs twice, and a stopword that is also a symbol.
fn scanned(name: &str) -> (PathBuf, Engine) {
    let root = std::env::temp_dir().join(format!("nexus-symptom-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    // The `mod.rs` files are load-bearing, not decoration. The Rust analyzer derives a
    // module's *scope* from the file path but only emits a module *symbol* from a `mod_item`
    // declaration — so without `pub mod cache;` this fixture holds `context::cache::put` and
    // no symbol named `cache` at all, and every test below would fail for a fixture reason
    // rather than a code one.
    for (path, body) in [
        (
            "src/lib.rs",
            "pub mod context;\npub mod store;\npub mod util;\npub mod a;\npub mod b;\n",
        ),
        ("src/context/mod.rs", "pub mod cache;\n"),
        ("src/context/cache.rs", "pub fn put() {}\npub fn get() {}\n"),
        ("src/store/mod.rs", "pub mod ledger;\n"),
        ("src/store/ledger.rs", "pub fn append() {}\n"),
        ("src/a/mod.rs", "pub mod handler;\n"),
        ("src/a/handler.rs", "pub fn handle_it() {}\n"),
        ("src/b/mod.rs", "pub mod handler;\n"),
        ("src/b/handler.rs", "pub fn handle_it() {}\n"),
        ("src/util/mod.rs", "pub mod error;\n"),
        ("src/util/error.rs", "pub fn report() {}\n"),
    ] {
        let p = root.join(path);
        fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        fs::write(p, body).expect("write");
    }
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "x"]);
    let (mut engine, _) = Engine::init(&root, nexus_lang_pack::default_registry()).expect("init");
    engine.scan().expect("scan");
    (root, engine)
}

fn files_in(engine: &Engine, text: &str) -> Vec<String> {
    let mut r = TaskRequest::session(TASK_BUDGET_TOKENS);
    r.text = text.into();
    r.purpose = Purpose::Task;
    engine
        .context(&r)
        .expect("context")
        .items
        .iter()
        .map(|i| i.anchor.file.clone())
        .collect()
}

#[test]
fn a_symptom_in_plain_words_reaches_the_code_it_names() {
    let (_root, engine) = scanned("plain");
    let files = files_in(
        &engine,
        "the context cache serves a package from before a fact was recorded",
    );
    assert!(
        files.iter().any(|f| f.contains("context/cache.rs")),
        "`cache` is one indexed symbol and the symptom names it: {files:?}"
    );
}

#[test]
fn a_word_naming_two_symbols_seeds_both_of_them() {
    // Was `a_word_naming_two_symbols_seeds_nothing`, asserting the opposite. Ambiguity is now
    // a discount, not a disqualification (see `an_ambiguous_exact_name_seeds_every_match_up_to_the_cap`
    // below, which pins this down at the seed-strength level rather than the file list) —
    // corrected rather than deleted, so the fixture's `handler` collision keeps a regression
    // test at this level too.
    let (_root, engine) = scanned("ambiguous");
    let files = files_in(&engine, "the handler drops the second request");
    assert!(
        files.iter().any(|f| f.contains("a/handler.rs"))
            && files.iter().any(|f| f.contains("b/handler.rs")),
        "`handler` names two symbols, one in each file; ambiguity is a discount, not a \
         disqualification, so both must seed, not just one: {files:?}"
    );
}

fn seeds_for(engine: &Engine, text: &str) -> Vec<Seed> {
    let mut r = TaskRequest::session(TASK_BUDGET_TOKENS);
    r.text = text.into();
    r.purpose = Purpose::Task;
    engine.seeds(&r, Intent::Debug).expect("seeds").seeds
}

/// A word naming two or three symbols seeds all of them; a word naming a language idiom's
/// every definition seeds none.
///
/// Both halves are the requirement. Without the first, `semaphore` and `framed` contribute
/// nothing to the prompts they are the subject of. Without the second, `drop` — the exact
/// name of 104 symbols in tokio — would seed all of them and expand from each, inside a hook
/// with a 150 ms budget. Those counts are tokio's; this fixture's own collision is `handler`
/// (`a::handler`, `b::handler` — see `scanned`'s doc comment above), the exact name of two
/// symbols and nothing more.
///
/// No fixture generated by any *other* helper in this file is the exact name of more than
/// `AMBIGUOUS_NAME_CAP` (4) symbols — the widest one already in use is this fixture's own
/// `handler` at two. The over-cap half is `an_exact_name_over_the_cap_seeds_nothing_and_says_why`
/// below, against `scanned_over_cap`, a fixture built for that width the same way
/// `scanned_crowded` further down is built for `WORD_HIT_LIMIT` and `TOKEN_FAMILY_NAME_CAP`.
#[test]
fn an_ambiguous_exact_name_seeds_every_match_up_to_the_cap() {
    let (_root, engine) = scanned("handler-cap");
    let seeds = seeds_for(&engine, "the handler drops the second request");

    let graded: Vec<_> = seeds
        .iter()
        .filter(|s| s.strength == SeedStrength::ProseAmbiguous)
        .collect();
    assert!(
        graded.len() >= 2,
        "every match must seed, not one of them: {seeds:?}"
    );
    assert!(
        graded.iter().all(|s| s.why.contains("names")),
        "the explanation must say the name was ambiguous: {graded:?}"
    );
}

/// A fixture with one word that is the exact name of more symbols than `AMBIGUOUS_NAME_CAP`
/// (4) admits — tokio's real case is `drop` at 104; this is the same shape at the smallest
/// width (5) that actually clears the cap. Built the same way `scanned_crowded` further down
/// is built for `WORD_HIT_LIMIT` and `TOKEN_FAMILY_NAME_CAP`: synthetic content sized to a
/// threshold, not a fixture invented to fit a number where an organic one was expected — there
/// is no organic five-way exact-name collision anywhere else in this file to reuse instead.
fn scanned_over_cap(name: &str) -> (PathBuf, Engine) {
    let root = std::env::temp_dir().join(format!("nexus-overcap-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let mut lib = String::new();
    let mut files = Vec::new();
    for i in 0..5 {
        lib.push_str(&format!("pub mod m{i};\n"));
        files.push((format!("src/m{i}.rs"), "pub fn drop() {}\n".to_string()));
    }
    files.insert(0, ("src/lib.rs".to_string(), lib));
    for (path, body) in &files {
        let p = root.join(path);
        fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        fs::write(p, body).expect("write");
    }
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "x"]);
    let (mut engine, _) = Engine::init(&root, nexus_lang_pack::default_registry()).expect("init");
    engine.scan().expect("scan");
    (root, engine)
}

/// The half `an_ambiguous_exact_name_seeds_every_match_up_to_the_cap` cannot exercise, because
/// no fixture in this file otherwise clears the cap.
///
/// This drives `resolve` through `Engine::seeds` for real — unlike
/// `context::seeds::tests::exactly_named_over_the_cap_is_counted_correctly`, which only checks
/// `exactly_named`'s count against the constant and would keep passing even if the guard that
/// reads that count were deleted. Here, deleting the guard would seed all five `drop` matches
/// at `ProseAmbiguous` and drop the exclusion note, and both assertions below would fail.
#[test]
fn an_exact_name_over_the_cap_seeds_nothing_and_says_why() {
    let (_root, engine) = scanned_over_cap("drop");
    let mut r = TaskRequest::session(TASK_BUDGET_TOKENS);
    r.text = "the resource does not drop cleanly under load".into();
    r.purpose = Purpose::Task;
    let result = engine.seeds(&r, Intent::Debug).expect("seeds");

    assert!(
        result.seeds.is_empty(),
        "`drop` is the exact name of five symbols here, one more than the cap: nothing else in \
         this fixture or prompt can seed, so the observable consequence of the refusal is an \
         empty seed list: {:?}",
        result.seeds
    );
    assert!(
        result
            .notes
            .iter()
            .any(|n| n.contains("too many to tell apart")),
        "the exclusion must be recorded, not silent: {:?}",
        result.notes
    );
}

#[test]
fn a_stopword_seeds_nothing_even_when_it_is_a_symbol() {
    // `error` is in the index here. It is also the word every symptom in the world contains.
    let (_root, engine) = scanned("stopword");
    let files = files_in(&engine, "there is an error when the ledger appends");
    assert!(
        !files.iter().any(|f| f.contains("util/error.rs")),
        "a stopword is not a seed however well it matches: {files:?}"
    );
    assert!(
        files.iter().any(|f| f.contains("store/ledger.rs")),
        "the distinctive word in the same sentence still seeds: {files:?}"
    );
}

/// A prompt is not the 25-word symptom sentence `targets` budgets for.
///
/// Paste a stack trace, a diff or a file into one and every distinct word becomes a candidate:
/// one indexed lookup each, and one compound-`SELECT` arm each in the fact query, which SQLite
/// refuses past 500 terms. That was a hard error out of `nexus context` — and the
/// `UserPromptSubmit` hook that runs it discards stderr, so a long prompt arrived as no
/// context at all rather than as a diagnostic anyone could act on.
#[test]
fn a_pasted_prompt_is_answered_rather_than_refused() {
    let (_root, engine) = scanned("pasted");
    // 800 distinct identifier-shaped words: past SEED_QUERY_CAP (256) and past
    // SQLITE_MAX_COMPOUND_SELECT (500 terms), so the cap and the ceiling it protects are
    // both exercised. Identifier-shaped, not prose, because prose is what the cap drops
    // first — a test that fed it prose would stop covering the seed query the day the
    // ordering rule changed.
    let pasted: String = (0..800).map(|i| format!("Widget{i}_field ")).collect();
    let mut r = TaskRequest::session(TASK_BUDGET_TOKENS);
    r.text = pasted;
    r.purpose = Purpose::Task;
    let pkg = engine
        .context(&r)
        .expect("a long prompt is capped, not failed");
    assert!(
        pkg.notes.iter().any(|n| n.contains("candidate words")),
        "the cap bit, so the package has to say so: {:?}",
        pkg.notes
    );
}

#[test]
fn a_short_word_seeds_nothing() {
    // Still an empty package, and for two reasons now rather than one. These words seed
    // nothing, as ever — and they do not reach the lexical fallback either, because a
    // three-letter word is below the length floor the seeder itself applies, so the prompt
    // corroborates nothing to fall back on. `get` and `put` are in the body of `cache.rs`,
    // so a gate that looked only at whether BM25 matched would send it.
    let (_root, engine) = scanned("short");
    let pkg = package(&engine, "get put now");
    assert!(
        pkg.items.is_empty(),
        "three-letter words are not evidence, however many symbols they match: {:?}",
        pkg.items.iter().map(|i| &i.anchor.file).collect::<Vec<_>>()
    );
    assert!(
        pkg.notes.iter().any(|n| n.contains("no seed")),
        "and the package still has to say nothing anchored: {:?}",
        pkg.notes
    );
}

/// A fixture built for one collision: `resolved` the function, three lines under `Resolved`
/// the enum. `find_symbols` matches case-insensitively, so a word that counts arity before
/// filtering by exact `last_segment` sees two hits for `resolved` and refuses both — even
/// though only one of them actually spells it that way.
fn scanned_case_variant() -> (PathBuf, Engine) {
    let root = std::env::temp_dir().join(format!("nexus-symptom-case-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let p = root.join("src/lib.rs");
    fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    fs::write(
        &p,
        "pub enum Resolved { Yes, No }\npub fn resolved() -> Resolved { Resolved::Yes }\n",
    )
    .expect("write");
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "x"]);
    let (mut engine, _) = Engine::init(&root, nexus_lang_pack::default_registry()).expect("init");
    engine.scan().expect("scan");
    (root, engine)
}

#[test]
fn a_case_variant_elsewhere_does_not_hide_a_real_unique_match() {
    let (_root, engine) = scanned_case_variant();
    let files = files_in(&engine, "the request is resolved before the retry finishes");
    assert!(
        files.iter().any(|f| f.contains("src/lib.rs")),
        "`resolved` uniquely names the function even though `Resolved` also matches it \
         case-insensitively: {files:?}"
    );
}

/// A fixture with prose in it, because the fallback ranks file *contents*.
///
/// `scanned` above is all one-line bodies (`pub fn put() {}`), which is right for testing
/// what seeds and wrong for testing what happens when nothing does: BM25 reads bodies, not
/// paths, so a no-seed prompt against that fixture finds nothing and a test built on it would
/// pass for the wrong reason. This one carries the shape the benchmark actually met — a
/// symptom whose words appear in a comment, naming no symbol at all.
fn scanned_prose(name: &str) -> (PathBuf, Engine) {
    let root = std::env::temp_dir().join(format!("nexus-prose-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    for (path, body) in [
        ("src/lib.rs", "pub mod pay;\npub mod cache;\n"),
        (
            "src/pay.rs",
            "/// Widen the idempotency key column to 128 characters everywhere it is\n\
             /// constrained. The upstream provider sends longer keys now.\n\
             pub fn charge() {}\n",
        ),
        (
            "src/cache.rs",
            "/// Unrelated: eviction, timers, and a ring buffer.\npub fn evict() {}\n",
        ),
    ] {
        let p = root.join(path);
        fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        fs::write(p, body).expect("write");
    }
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "x"]);
    let (mut engine, _) = Engine::init(&root, nexus_lang_pack::default_registry()).expect("init");
    engine.scan().expect("scan");
    (root, engine)
}

fn package(engine: &Engine, text: &str) -> nexus_core::context::ContextPackage {
    let mut r = TaskRequest::session(TASK_BUDGET_TOKENS);
    r.text = text.into();
    r.purpose = Purpose::Task;
    engine.context(&r).expect("context")
}

#[test]
fn a_prompt_that_anchors_nothing_ranks_file_contents_instead() {
    // The measured case: a symptom written in prose, naming no symbol. "idempotency" and
    // "column" are ordinary lowercase words that name nothing in the index, so seeding
    // anchors nothing — and returning an empty package here is what sent an agent into a
    // task with no context at all on two of five real benchmark prompts.
    let (_root, engine) = scanned_prose("fires");
    let pkg = package(
        &engine,
        "the idempotency key column is too short for the upstream provider",
    );

    assert!(
        !pkg.items.is_empty(),
        "nothing anchored, so the fallback should have ranked file contents: {pkg:?}"
    );
    assert!(
        pkg.items.iter().any(|i| i.anchor.file.contains("pay.rs")),
        "the file whose text the symptom describes: {:?}",
        pkg.items.iter().map(|i| &i.anchor.file).collect::<Vec<_>>()
    );
    assert!(
        pkg.items.iter().all(|i| i.why.starts_with("bm25")),
        "every item must say it is a lexical guess, not a graph answer: {:?}",
        pkg.items.iter().map(|i| &i.why).collect::<Vec<_>>()
    );
    assert!(
        pkg.notes.iter().any(|n| n.contains("no seed")),
        "the agent still has to learn nothing anchored: {:?}",
        pkg.notes
    );
}

#[test]
fn a_prompt_that_does_anchor_is_untouched_by_the_fallback() {
    // The trigger is "seeding anchored nothing" and nothing wider. If it ever fires for a
    // prompt that did anchor, every package the product produces changes and no regression
    // is attributable to anything.
    let (_root, engine) = scanned_prose("narrow");
    let pkg = package(&engine, "charge is called twice");

    assert!(
        !pkg.items.is_empty(),
        "`charge` names exactly one symbol, so this must anchor: {pkg:?}"
    );
    assert!(
        pkg.items.iter().all(|i| !i.why.starts_with("bm25")),
        "a prompt that anchored must not be answered lexically: {:?}",
        pkg.items.iter().map(|i| &i.why).collect::<Vec<_>>()
    );
    assert!(
        !pkg.notes.iter().any(|n| n.contains("no seed")),
        "and must not claim nothing anchored: {:?}",
        pkg.notes
    );
}

/// A fixture shaped like the two benchmark repositories the seed stage went silent on.
///
/// Java, because the defect is about camelCase: a prompt says "the idempotency key" and "the
/// total", and the index holds `idempotencyKey` and `getTotalAmount`. Rust fixtures cannot
/// reproduce it — `snake_case` puts the word at a separator the old suffix match already saw.
///
/// Three properties are load-bearing and are why this is not smaller:
///   * `idempotency` is a token of exactly four names, spread over two files — a family;
///   * `payment` is a token of nine, which is a theme and must still seed nothing;
///   * `orders` is the whole name of two symbols — the ambiguous-name discount seeds both of
///     them, weakly, rather than the disqualification this fixture predates.
fn scanned_camel(name: &str) -> (PathBuf, Engine) {
    let root = std::env::temp_dir().join(format!("nexus-camel-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    for (path, body) in [
        (
            "pom.xml",
            "<project><modelVersion>4.0.0</modelVersion><groupId>mn</groupId>\
             <artifactId>demo</artifactId><version>1</version></project>\n",
        ),
        (
            "src/main/java/mn/pay/Payment.java",
            "package mn.pay;\npublic class Payment {\n  private String idempotencyKey;\n  \
             public String getIdempotencyKey() { return idempotencyKey; }\n}\n",
        ),
        (
            "src/main/java/mn/pay/PaymentRepository.java",
            "package mn.pay;\npublic interface PaymentRepository {\n  \
             boolean existsByIdempotencyKey(String k);\n  \
             Payment findByIdempotencyKey(String k);\n}\n",
        ),
        (
            "src/main/java/mn/pay/PaymentService.java",
            "package mn.pay;\npublic class PaymentService {\n  public PaymentService() {}\n  \
             public void createPayment() {}\n}\n",
        ),
        (
            "src/main/java/mn/pay/PaymentValidator.java",
            "package mn.pay;\npublic class PaymentValidator {\n  public PaymentValidator() {}\n}\n",
        ),
        (
            "src/main/java/mn/pay/PaymentDto.java",
            "package mn.pay;\npublic class PaymentDto {\n  public PaymentDto() {}\n}\n",
        ),
        (
            "src/main/java/mn/shop/Order.java",
            "package mn.shop;\npublic class Order {\n  private java.math.BigDecimal gross;\n  \
             public java.math.BigDecimal getTotalAmount() { return gross; }\n}\n",
        ),
        (
            "src/main/java/mn/shop/OrderController.java",
            "package mn.shop;\npublic class OrderController {\n  \
             public java.util.List<Order> orders() { return null; }\n}\n",
        ),
        (
            "src/main/java/mn/shop/OrderReportController.java",
            "package mn.shop;\npublic class OrderReportController {\n  \
             public java.util.List<Order> orders() { return null; }\n}\n",
        ),
    ] {
        let p = root.join(path);
        fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        fs::write(p, body).expect("write");
    }
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "x"]);
    let (mut engine, _) = Engine::init(&root, nexus_lang_pack::default_registry()).expect("init");
    engine.scan().expect("scan");
    (root, engine)
}

/// `A1-idempotency-key-length`, the prompt verbatim.
///
/// `key` is below the length floor and never reaches the index; `idempotency` is a *prefix* of
/// `idempotencyKey`, and the suffix match the index used to run could not see it. Every other
/// word in the sentence names nothing, so the whole prompt anchored nothing and the agent was
/// handed a lexical guess over a repository it had not been told anything about.
#[test]
fn a_prompt_naming_a_camel_case_field_in_prose_anchors_on_it() {
    let (_root, engine) = scanned_camel("idempotency");
    let pkg = package(
        &engine,
        "The idempotency key column is too short for the new upstream provider. \
         Widen it to 128 characters everywhere it is constrained.",
    );
    let files: Vec<&String> = pkg.items.iter().map(|i| &i.anchor.file).collect();

    assert!(
        files.iter().any(|f| f.ends_with("Payment.java")),
        "`idempotency` names the field the prompt is about: {files:?}"
    );
    assert!(
        files.iter().any(|f| f.ends_with("PaymentRepository.java")),
        "and the two repository methods constrained by the same column: {files:?}"
    );
    assert!(
        pkg.items.iter().all(|i| !i.why.starts_with("bm25")),
        "the prompt anchored, so this must be a graph answer and not a lexical guess: {:?}",
        pkg.items.iter().map(|i| &i.why).collect::<Vec<_>>()
    );
}

/// `B2-orphaned-field-diagnosis`, the prompt verbatim — the live one, from
/// `tests/fixtures/specs/next-storefront/fixture.toml`.
///
/// `orders` is the whole name of two symbols, so the ambiguous-name discount seeds both of
/// them weakly; what actually anchors this prompt is `total`, an interior token of
/// `getTotalAmount` — the field whose rename is the bug being reported.
///
/// The sentence moved when the task's start commit did (it read "The orders page shows NaN for
/// every total" against c3). It is pinned to the live wording rather than the historical one on
/// purpose: a regression test for "the prompts the benchmark actually sends still seed" that
/// quotes a prompt nothing sends any more passes whether or not the thing works, which is the
/// defect class this branch exists to remove.
#[test]
fn a_symptom_naming_an_interior_token_anchors_on_the_field_it_describes() {
    let (_root, engine) = scanned_camel("total");
    let pkg = package(
        &engine,
        "The orders page is broken — it throws instead of rendering the total for each order. \
         Find out why and fix it.",
    );
    let files: Vec<&String> = pkg.items.iter().map(|i| &i.anchor.file).collect();

    assert!(
        files.iter().any(|f| f.ends_with("Order.java")),
        "`total` is a word in `getTotalAmount` and nothing else in the index: {files:?}"
    );
    assert!(
        pkg.items.iter().all(|i| !i.why.starts_with("bm25")),
        "the prompt anchored, so this must be a graph answer and not a lexical guess: {:?}",
        pkg.items.iter().map(|i| &i.why).collect::<Vec<_>>()
    );
}

/// The bound. A word can be a token of half the repository, and then it names nothing.
///
/// `payment` is a token of nine names here — four classes, an interface, the three declared
/// constructors and `createPayment`. Seeding that is not a smaller package than seeding the
/// repository, it
/// is the same package with a story attached, so the word is refused and the request falls
/// back to a guess that says out loud that it is one.
#[test]
fn a_word_that_names_half_the_repository_seeds_nothing() {
    let (_root, engine) = scanned_camel("theme");
    let pkg = package(&engine, "the payment behaves oddly under load");

    assert!(
        pkg.items.iter().all(|i| i.why.starts_with("bm25")),
        "`payment` is a theme, not a name: nothing may anchor on it: {:?}",
        pkg.items
            .iter()
            .map(|i| (&i.anchor.file, &i.why))
            .collect::<Vec<_>>()
    );
    assert!(
        pkg.notes.iter().any(|n| n.contains("no seed")),
        "and the package has to say nothing anchored: {:?}",
        pkg.notes
    );
}

/// A fixture where one word has a crowd of near-misses around it.
///
/// The index is asked about a word with a single SQL query, and that query over-matches on
/// purpose: `LIKE '%unit%'` also returns `unit_1`, and `LIKE '%order%'` also returns `reorder`.
/// Both halves of that bill come due only at scale, so the crowd here is a real one:
///
///   * 210 symbols named `unit_N`, all with shorter FQNs than the one symbol actually called
///     `unit`, which sits three modules down. They are past `WORD_HIT_LIMIT`, so ordering by
///     length alone would return 200 near-misses and evict the answer.
///   * 8 symbols whose names carry `order` as a token, sitting behind 20 named `reorder_N`
///     that merely contain it. A window too small to hold both returns a *diluted* family that
///     is under the cap, and the refusal becomes a function of row order.
fn scanned_crowded(name: &str) -> (PathBuf, Engine) {
    let root = std::env::temp_dir().join(format!("nexus-crowd-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let mut lib = String::from("pub mod deep;\npub mod wide;\n");
    for i in 0..210 {
        lib.push_str(&format!("pub fn unit_{i}() {{}}\n"));
    }
    for i in 0..20 {
        lib.push_str(&format!("pub fn reorder_{i}() {{}}\n"));
    }
    let mut members = String::new();
    for i in 0..8 {
        members.push_str(&format!("pub fn order_b{i}() {{}}\n"));
    }
    for (path, body) in [
        ("src/lib.rs", lib.as_str()),
        ("src/deep/mod.rs", "pub mod inner;\n"),
        ("src/deep/inner.rs", "pub fn unit() {}\n"),
        ("src/wide/mod.rs", "pub mod members;\n"),
        ("src/wide/members.rs", members.as_str()),
    ] {
        let p = root.join(path);
        fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        fs::write(p, body).expect("write");
    }
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "x"]);
    let (mut engine, _) = Engine::init(&root, nexus_lang_pack::default_registry()).expect("init");
    engine.scan().expect("scan");
    (root, engine)
}

/// Widening the query must not lose an answer the narrow one gave.
///
/// `find_symbols_by_word` admits every symbol whose name merely *contains* the word, and there
/// are 210 of those here against one symbol actually called `unit`. Ordered by FQN length the
/// near-misses fill the window and the real match never comes back — a word that anchored
/// before the widening would anchor no longer, which is a regression dressed as a feature.
/// The query sorts every row the narrow match would have returned to the front for this reason.
#[test]
fn a_crowd_of_near_misses_cannot_evict_the_symbol_a_word_actually_names() {
    let (_root, engine) = scanned_crowded("evict");
    let files = files_in(&engine, "the unit is reported twice");
    assert!(
        files.iter().any(|f| f.contains("deep/inner.rs")),
        "`unit` is the name of exactly one symbol, however many others merely contain it: \
         {files:?}"
    );
}

/// The cap has to see the family, not a sample of it.
///
/// `order` is a token of eight names here, which is over the cap and must seed nothing. Twenty
/// symbols named `reorder_N` merely contain the word and sort ahead of all eight. Judge the cap
/// against a window that holds only some of them and the family arrives under the cap and
/// seeds — the refusal would be decided by row order rather than by how wide the word is.
#[test]
fn a_wide_family_is_refused_even_when_near_misses_crowd_the_window() {
    let (_root, engine) = scanned_crowded("dilute");
    let pkg = package(&engine, "the order fails to save");
    assert!(
        pkg.items.iter().all(|i| i.why.starts_with("bm25")),
        "`order` is a token of eight names, which is a theme: nothing may anchor on it: {:?}",
        pkg.items
            .iter()
            .map(|i| (&i.anchor.file, &i.why))
            .collect::<Vec<_>>()
    );
    assert!(
        pkg.notes.iter().any(|n| n.contains("no seed")),
        "and the package has to say nothing anchored: {:?}",
        pkg.notes
    );
}

/// The stronger reading of a symbol wins whichever reading arrives first.
///
/// A seed's strength is decoupled from its `SeedSource`, and one source — `NameMatch` — carries
/// all three grades: a code-shaped word, a prose word naming exactly one symbol, and a prose
/// word that is only a token of some names. So `offer`'s merge cannot follow the source the way
/// `why` does; it takes the *stronger* strength. Without that, a symbol offered first as one
/// token of a family and then as a name someone typed would keep the 0.3 — the source did not
/// improve, so nothing would update — and the grade would depend on the order the words happen
/// to be sorted in.
///
/// Both orders are asserted, because a test that exercised only one would pass against exactly
/// the bug this rule exists to prevent. Candidate words are sorted, and ASCII puts a capital
/// before a lowercase letter, so the two prompts below deliver the two readings in opposite
/// orders: `idempotency` < `idempotencyKey`, but `getIdempotencyKey` < `idempotency`.
#[test]
fn the_stronger_reading_of_a_word_wins_whichever_arrives_first() {
    let (_root, engine) = scanned_camel("merge");

    for (order, text, fqn) in [
        (
            "weak first",
            "the idempotencyKey is reused whenever idempotency is retried",
            "#idempotencyKey",
        ),
        (
            "strong first",
            "getIdempotencyKey returns the wrong idempotency after a retry",
            "#getIdempotencyKey(",
        ),
    ] {
        let mut r = TaskRequest::session(TASK_BUDGET_TOKENS);
        r.text = text.into();
        r.purpose = Purpose::Task;
        let seeds = engine.seeds(&r, Intent::Debug).expect("seeds").seeds;
        let seed = seeds
            .iter()
            .find(|s| s.symbol.fqn.contains(fqn))
            .unwrap_or_else(|| panic!("{order}: {fqn} must seed at all: {seeds:?}"));
        assert_eq!(
            seed.strength,
            SeedStrength::CodeShape,
            "{order}: `idempotency` offers this symbol as one token of a four-name family, and \
             the code-shaped word names it outright. The stronger reading must survive the \
             merge: {seed:?}"
        );
        // The sentence has to survive the merge with the number it explains. Both readings
        // arrive as `NameMatch`, so a merge that replaced `why` only on a better *source*
        // would leave "is a word in the name …" standing under a 1.0 — and `--explain` is
        // where a person reads that, so a wrong sentence there is a wrong answer.
        assert!(
            !seed.why.contains("is a word in the name"),
            "{order}: the explanation must follow the strength it explains, not the weaker \
             reading that arrived first: {seed:?}"
        );
    }
}
