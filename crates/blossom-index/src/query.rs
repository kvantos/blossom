//! The Orgzly-compatible query language, ported from Bloom's `SearchQuery.kt`
//! (see Bloom's `docs/search-syntax.md`).
//!
//! Space or `AND` is AND, `OR` binds looser, `.` negates a term, `( … )`
//! groups, `.( … )` negates a group, `"…"` quotes. `o.PROP` sorts and
//! `ad.N` asks for the agenda view; both are directives, not terms. Parsing
//! never fails: a missing `)` or `"` ends at the end of the query, and a
//! stray `)`, an empty `()` or `""` is ignored.

use chrono::{Days, Months, NaiveDate, NaiveDateTime, Timelike};

/// A parsed query.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    /// The terms as a tree with negation pushed down to the leaves.
    /// `None` matches everything.
    pub expr: Option<Expr>,
    /// `o.PROP` directives, in order.
    pub sort: Vec<SortKey>,
    /// `ad.N`: the agenda view over N days starting today.
    pub agenda_days: Option<u32>,
}

/// A boolean query tree in negation normal form: only leaves carry a NOT.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    /// One term.
    Leaf(Term),
    /// All must match.
    And(Vec<Expr>),
    /// Any must match.
    Or(Vec<Expr>),
}

impl Expr {
    /// Every leaf, left to right.
    #[must_use]
    pub fn leaves(&self) -> Vec<&Term> {
        match self {
            Expr::Leaf(t) => vec![t],
            Expr::And(c) | Expr::Or(c) => c.iter().flat_map(Expr::leaves).collect(),
        }
    }

    /// Evaluates the tree with `test` deciding each leaf.
    pub fn matches(&self, test: &mut impl FnMut(&Term) -> bool) -> bool {
        match self {
            Expr::Leaf(t) => test(t),
            Expr::And(c) => c.iter().all(|e| e.matches(test)),
            Expr::Or(c) => c.iter().any(|e| e.matches(test)),
        }
    }

    fn negate(self) -> Expr {
        match self {
            Expr::Leaf(t) => Expr::Leaf(Term {
                negated: !t.negated,
                ..t
            }),
            Expr::And(c) => Expr::Or(c.into_iter().map(Expr::negate).collect()),
            Expr::Or(c) => Expr::And(c.into_iter().map(Expr::negate).collect()),
        }
    }
}

/// One condition, possibly negated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Term {
    /// What to test.
    pub condition: Condition,
    /// `.` prefix.
    pub negated: bool,
}

/// What `it.` tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateType {
    /// Any not-done keyword.
    Todo,
    /// Any done keyword.
    Done,
    /// No keyword.
    None,
}

/// One kind of term.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Condition {
    /// Plain text: case-insensitive substring of title, body or own tags.
    Text(String),
    /// `i.STATE`: that keyword (case-insensitive); `i.none` for none.
    State(String),
    /// `it.TYPE`.
    StateType(StateType),
    /// `b.NAME`: in that notebook.
    Notebook(String),
    /// `t.TAG` (inherited tags) or `tn.TAG` (own tags only); substring.
    Tag {
        /// The tag text.
        tag: String,
        /// `tn.`
        own_only: bool,
    },
    /// `p.X` (no priority counts as the default) or `ps.X` (written only).
    Priority {
        /// The priority.
        priority: String,
        /// `ps.`
        set_only: bool,
    },
    /// `s.DATE`
    Scheduled(Period),
    /// `d.DATE`
    Deadline(Period),
    /// `e.DATE` / `a.DATE`: a bare active timestamp (event).
    Event(Period),
    /// `c.DATE`
    Closed(Period),
    /// `cr.DATE`: the `CREATED` property.
    Created(Period),
    /// `id.X`: `:ID:` or `:CUSTOM_ID:` starts with X; `id.none` for neither.
    Id(String),
}

/// `eq ne lt le gt ge`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareOp {
    /// On.
    Eq,
    /// Not on.
    Ne,
    /// Before.
    Lt,
    /// On or before.
    Le,
    /// After.
    Gt,
    /// On or after.
    Ge,
}

impl CompareOp {
    fn parse(s: &str) -> Option<CompareOp> {
        Some(match s.to_ascii_lowercase().as_str() {
            "eq" => CompareOp::Eq,
            "ne" => CompareOp::Ne,
            "lt" => CompareOp::Lt,
            "le" => CompareOp::Le,
            "gt" => CompareOp::Gt,
            "ge" => CompareOp::Ge,
            _ => return None,
        })
    }

    /// Compares two days (or two moments).
    #[must_use]
    pub fn test<T: Ord>(self, value: &T, target: &T) -> bool {
        match self {
            CompareOp::Eq => value == target,
            CompareOp::Ne => value != target,
            CompareOp::Lt => value < target,
            CompareOp::Le => value <= target,
            CompareOp::Gt => value > target,
            CompareOp::Ge => value >= target,
        }
    }
}

/// A date token: `[OP.]TIME`, or the specials `overdue` and `none`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Period {
    /// TIME as written.
    pub raw: String,
    /// An explicit operator; `None` means the prefix's default.
    pub op: Option<CompareOp>,
}

/// What a [`Period`] names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// One day.
    Day(NaiveDate),
    /// One moment (`now`, `Nh`).
    Moment(NaiveDateTime),
    /// `none`/`no`/`nodate`: the timestamp is absent.
    NoDate,
    /// `overdue`: dated before today.
    Overdue,
    /// Nothing valid; matches nothing.
    Invalid,
}

impl Period {
    fn parse(value: &str) -> Period {
        if let Some((op, rest)) = value.split_once('.')
            && let Some(op) = CompareOp::parse(op)
            && !rest.is_empty()
        {
            return Period {
                raw: rest.to_string(),
                op: Some(op),
            };
        }
        Period {
            raw: value.to_string(),
            op: None,
        }
    }

    /// Resolves the token against `now`.
    #[must_use]
    pub fn target(&self, now: NaiveDateTime) -> Target {
        let today = now.date();
        let lower = self.raw.to_lowercase();
        if self.op.is_none() {
            match lower.as_str() {
                "none" | "no" | "nodate" => return Target::NoDate,
                "overdue" => return Target::Overdue,
                _ => {}
            }
        }
        let day = |d: Option<NaiveDate>| d.map_or(Target::Invalid, Target::Day);
        match lower.as_str() {
            "today" | "tod" => return Target::Day(today),
            "tomorrow" | "tom" | "tmrw" => return day(today.succ_opt()),
            "yesterday" => return day(today.pred_opt()),
            "now" => return to_minute(now).map_or(Target::Invalid, Target::Moment),
            _ => {}
        }
        if let Some((n, unit)) = signed_offset(&lower) {
            return match unit {
                'h' => to_minute(now)
                    .and_then(|m| m.checked_add_signed(chrono::TimeDelta::try_hours(n)?))
                    .map_or(Target::Invalid, Target::Moment),
                'd' => day(shift_days(today, n)),
                'w' => day(n.checked_mul(7).and_then(|d| shift_days(today, d))),
                'm' => day(shift_months(today, n)),
                'y' => day(n.checked_mul(12).and_then(|m| shift_months(today, m))),
                _ => Target::Invalid,
            };
        }
        NaiveDate::parse_from_str(&self.raw, "%Y-%m-%d").map_or(Target::Invalid, Target::Day)
    }
}

fn to_minute(t: NaiveDateTime) -> Option<NaiveDateTime> {
    t.with_second(0)?.with_nanosecond(0)
}

/// `[+-]N[hdwmy]`.
fn signed_offset(s: &str) -> Option<(i64, char)> {
    let unit = s.chars().last()?;
    if !"hdwmy".contains(unit) {
        return None;
    }
    let num = &s[..s.len() - 1];
    let digits = num.strip_prefix(['+', '-']).unwrap_or(num);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((num.parse().ok()?, unit))
}

fn shift_days(d: NaiveDate, n: i64) -> Option<NaiveDate> {
    let abs = Days::new(n.unsigned_abs());
    if n >= 0 {
        d.checked_add_days(abs)
    } else {
        d.checked_sub_days(abs)
    }
}

/// Calendar months, clamping the day as java.time's `plusMonths` does.
fn shift_months(d: NaiveDate, n: i64) -> Option<NaiveDate> {
    let abs = Months::new(u32::try_from(n.unsigned_abs()).ok()?);
    if n >= 0 {
        d.checked_add_months(abs)
    } else {
        d.checked_sub_months(abs)
    }
}

/// What `o.PROP` sorts by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortField {
    /// `b`, `book`, `notebook`
    Notebook,
    /// `t`, `title`
    Title,
    /// `s`, `sched`, `scheduled`
    Scheduled,
    /// `d`, `dead`, `deadline`
    Deadline,
    /// `e`, `event`, `a`, `active`
    Event,
    /// `c`, `close`, `closed`
    Closed,
    /// `cr`, `created`
    Created,
    /// `p`, `pri`, `prio`, `priority`
    Priority,
    /// `st`, `state`
    State,
}

impl SortField {
    fn parse(name: &str) -> Option<SortField> {
        Some(match name.to_lowercase().as_str() {
            "b" | "book" | "notebook" => SortField::Notebook,
            "t" | "title" => SortField::Title,
            "s" | "sched" | "scheduled" => SortField::Scheduled,
            "d" | "dead" | "deadline" => SortField::Deadline,
            "e" | "event" | "a" | "active" => SortField::Event,
            "c" | "close" | "closed" => SortField::Closed,
            "cr" | "created" => SortField::Created,
            "p" | "pri" | "prio" | "priority" => SortField::Priority,
            "st" | "state" => SortField::State,
            _ => return None,
        })
    }
}

/// One `o.PROP` (ascending) or `.o.PROP` (descending).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SortKey {
    /// The property.
    pub field: SortField,
    /// `.o.`
    pub descending: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    /// A word; `literal` when it started with a quote (always plain text).
    Word {
        text: String,
        literal: bool,
    },
    Open,
    Close,
    /// A `.` directly before `(`.
    Not,
    Or,
    And,
}

impl Query {
    /// Parses `input`. Never fails.
    #[must_use]
    pub fn parse(input: &str) -> Query {
        let mut p = Parser {
            tokens: tokenize(input),
            pos: 0,
            depth: 0,
            query: Query::default(),
        };
        let expr = p.expr();
        Query { expr, ..p.query }
    }

    /// Whether the query has no terms and no agenda directive.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.expr.is_none() && self.agenda_days.is_none()
    }

    /// Splits the query for semantic search into the words to rank by
    /// meaning and the query that still filters.
    ///
    /// The words are the positive plain-text terms every match must have:
    /// those joined by AND at the top, including inside nested AND groups,
    /// in query order, joined by spaces (a quoted phrase stays one piece).
    /// Everything else stays in the filter: tags, states, dates, negated
    /// words, and words inside an OR (taking those out would change what
    /// matches), plus `o.` sorting and `ad.`. An empty string means there
    /// is nothing to rank by meaning.
    ///
    /// `buy milk t.home` gives the words `buy milk` and the filter
    /// `t.home`; `milk OR tea` gives no words and the whole query back.
    #[must_use]
    pub fn split_text(&self) -> (String, Query) {
        let mut words = Vec::new();
        let expr = self.expr.clone().and_then(|e| take_words(e, &mut words));
        (
            words.join(" "),
            Query {
                expr,
                ..self.clone()
            },
        )
    }

    /// The positive plain-text terms, for narrowing and ranking.
    #[must_use]
    pub fn text_terms(&self) -> Vec<&str> {
        self.expr.as_ref().map_or_else(Vec::new, |e| {
            e.leaves()
                .into_iter()
                .filter(|t| !t.negated)
                .filter_map(|t| match &t.condition {
                    Condition::Text(s) => Some(s.as_str()),
                    _ => None,
                })
                .collect()
        })
    }
}

/// `expr` without its required positive text terms, which go to `words`;
/// `None` when nothing is left. Doesn't look inside OR.
fn take_words(expr: Expr, words: &mut Vec<String>) -> Option<Expr> {
    match expr {
        Expr::Leaf(Term {
            condition: Condition::Text(t),
            negated: false,
        }) => {
            words.push(t);
            None
        }
        Expr::And(children) => {
            let mut rest: Vec<Expr> = children
                .into_iter()
                .filter_map(|c| take_words(c, words))
                .collect();
            match rest.len() {
                0 => None,
                1 => rest.pop(),
                _ => Some(Expr::And(rest)),
            }
        }
        other => Some(other),
    }
}

fn tokenize(input: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut literal = false;
    let mut had_quote = false;
    let flush =
        |tokens: &mut Vec<Token>, word: &mut String, literal: &mut bool, had_quote: &mut bool| {
            if word.is_empty() && !*had_quote {
                return;
            }
            let text = std::mem::take(word);
            tokens.push(if *had_quote {
                Token::Word {
                    text,
                    literal: *literal,
                }
            } else if text.eq_ignore_ascii_case("OR") {
                Token::Or
            } else if text.eq_ignore_ascii_case("AND") {
                Token::And
            } else {
                Token::Word {
                    text,
                    literal: false,
                }
            });
            *literal = false;
            *had_quote = false;
        };
    for ch in input.chars() {
        if ch == '"' {
            if !quoted && (word.is_empty() || word == ".") {
                literal = true;
            }
            quoted = !quoted;
            had_quote = true;
        } else if quoted {
            word.push(ch);
        } else if ch.is_whitespace() {
            flush(&mut tokens, &mut word, &mut literal, &mut had_quote);
        } else if ch == '(' {
            if word == "." && !had_quote {
                word.clear();
                tokens.push(Token::Not);
            } else {
                flush(&mut tokens, &mut word, &mut literal, &mut had_quote);
            }
            tokens.push(Token::Open);
        } else if ch == ')' {
            flush(&mut tokens, &mut word, &mut literal, &mut had_quote);
            tokens.push(Token::Close);
        } else {
            word.push(ch);
        }
    }
    flush(&mut tokens, &mut word, &mut literal, &mut had_quote);
    tokens
}

/// Recursive descent over
/// `expr := and ("OR" and)*`, `and := unary (["AND"] unary)*`,
/// `unary := "." "(" expr ")" | "(" expr ")" | word`.
struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    depth: usize,
    query: Query,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn expr(&mut self) -> Option<Expr> {
        let mut parts: Vec<Expr> = self.and().into_iter().collect();
        while self.peek() == Some(&Token::Or) {
            self.pos += 1;
            parts.extend(self.and());
        }
        collapse(parts, Expr::Or)
    }

    fn and(&mut self) -> Option<Expr> {
        let mut parts = Vec::new();
        loop {
            match self.peek() {
                None | Some(Token::Or) => break,
                Some(Token::Close) => {
                    if self.depth > 0 {
                        break;
                    }
                    self.pos += 1;
                }
                Some(Token::And) => self.pos += 1,
                Some(_) => parts.extend(self.unary()),
            }
        }
        collapse(parts, Expr::And)
    }

    fn unary(&mut self) -> Option<Expr> {
        let token = self.tokens.get(self.pos)?.clone();
        self.pos += 1;
        match token {
            // The tokenizer only emits Not right before Open.
            Token::Not => self.unary().map(Expr::negate),
            Token::Open => {
                self.depth += 1;
                let inner = self.expr();
                if self.peek() == Some(&Token::Close) {
                    self.pos += 1;
                }
                self.depth -= 1;
                inner
            }
            Token::Word {
                text,
                literal: true,
            } => {
                let (negated, body) = match text.strip_prefix('.') {
                    Some(b) => (true, b),
                    None => (false, text.as_str()),
                };
                (!body.is_empty()).then(|| {
                    Expr::Leaf(Term {
                        condition: Condition::Text(body.to_string()),
                        negated,
                    })
                })
            }
            Token::Word {
                text,
                literal: false,
            } => self.word(&text),
            Token::Close | Token::Or | Token::And => None,
        }
    }

    fn word(&mut self, token: &str) -> Option<Expr> {
        // `.5` is text, not a negated `5`.
        let negated = token.starts_with('.')
            && token[1..]
                .chars()
                .next()
                .is_some_and(|c| !c.is_ascii_digit());
        let body = if negated { &token[1..] } else { token };
        let lower = body.to_lowercase();
        if let Some(prop) = lower.strip_prefix("o.") {
            if let Some(field) = SortField::parse(prop) {
                self.query.sort.push(SortKey {
                    field,
                    descending: negated,
                });
            }
            return None;
        }
        if let Some(n) = lower.strip_prefix("ad.") {
            self.query.agenda_days = n.parse().ok();
            return None;
        }
        Some(Expr::Leaf(Term {
            condition: condition(body),
            negated,
        }))
    }
}

fn collapse(mut parts: Vec<Expr>, make: fn(Vec<Expr>) -> Expr) -> Option<Expr> {
    match parts.len() {
        0 => None,
        1 => parts.pop(),
        _ => Some(make(parts)),
    }
}

fn condition(body: &str) -> Condition {
    if let Some(dot) = body.find('.')
        && (1..=2).contains(&dot)
    {
        let prefix = body[..dot].to_lowercase();
        let value = &body[dot + 1..];
        if !value.is_empty() {
            let v = value.to_string();
            match prefix.as_str() {
                "i" => return Condition::State(v),
                "it" => {
                    let t = match value.to_lowercase().as_str() {
                        "todo" => Some(StateType::Todo),
                        "done" => Some(StateType::Done),
                        "none" => Some(StateType::None),
                        _ => None,
                    };
                    if let Some(t) = t {
                        return Condition::StateType(t);
                    }
                }
                "b" => return Condition::Notebook(v),
                "t" => {
                    return Condition::Tag {
                        tag: v,
                        own_only: false,
                    };
                }
                "tn" => {
                    return Condition::Tag {
                        tag: v,
                        own_only: true,
                    };
                }
                "p" => {
                    return Condition::Priority {
                        priority: v,
                        set_only: false,
                    };
                }
                "ps" => {
                    return Condition::Priority {
                        priority: v,
                        set_only: true,
                    };
                }
                "s" => return Condition::Scheduled(Period::parse(value)),
                "d" => return Condition::Deadline(Period::parse(value)),
                "a" | "e" => return Condition::Event(Period::parse(value)),
                "c" => return Condition::Closed(Period::parse(value)),
                "cr" => return Condition::Created(Period::parse(value)),
                "id" => return Condition::Id(v),
                _ => {}
            }
        }
    }
    Condition::Text(body.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(c: Condition, negated: bool) -> Expr {
        Expr::Leaf(Term {
            condition: c,
            negated,
        })
    }

    fn text(s: &str) -> Condition {
        Condition::Text(s.into())
    }

    #[test]
    fn and_or_precedence() {
        let q = Query::parse("a b OR c AND d");
        assert_eq!(
            q.expr,
            Some(Expr::Or(vec![
                Expr::And(vec![leaf(text("a"), false), leaf(text("b"), false)]),
                Expr::And(vec![leaf(text("c"), false), leaf(text("d"), false)]),
            ]))
        );
        assert_eq!(Query::parse("a or b"), Query::parse("a OR b"));
    }

    #[test]
    fn negated_group_uses_de_morgan() {
        let q = Query::parse(".(t.work OR t.home) i.todo");
        let tag = |t: &str| Condition::Tag {
            tag: t.into(),
            own_only: false,
        };
        assert_eq!(
            q.expr,
            Some(Expr::And(vec![
                Expr::And(vec![leaf(tag("work"), true), leaf(tag("home"), true)]),
                leaf(Condition::State("todo".into()), false),
            ]))
        );
    }

    #[test]
    fn split_text_takes_required_words() {
        let split = |q: &str| {
            let (words, rest) = Query::parse(q).split_text();
            (words, rest.expr)
        };
        let tag = |t: &str| {
            leaf(
                Condition::Tag {
                    tag: t.into(),
                    own_only: false,
                },
                false,
            )
        };
        assert_eq!(split("buy milk"), ("buy milk".into(), None));
        assert_eq!(
            split("buy t.home milk"),
            ("buy milk".into(), Some(tag("home")))
        );
        assert_eq!(split("\"oat milk\" fresh"), ("oat milk fresh".into(), None));
        // Inside nested AND groups too, but never inside OR or when negated.
        assert_eq!(
            split("(milk t.home) .bread"),
            (
                "milk".into(),
                Some(Expr::And(vec![tag("home"), leaf(text("bread"), true)]))
            )
        );
        assert_eq!(
            split("milk OR tea"),
            (String::new(), Query::parse("milk OR tea").expr)
        );
        assert_eq!(
            split("plan (t.work OR urgent)"),
            (
                "plan".into(),
                Some(Expr::Or(vec![tag("work"), leaf(text("urgent"), false)]))
            )
        );
        assert_eq!(split("i.todo").0, "");
        assert_eq!(split(""), (String::new(), None));
        // Directives stay with the filter.
        let (words, rest) = Query::parse("ideas o.title ad.3").split_text();
        assert_eq!(words, "ideas");
        assert_eq!((rest.sort.len(), rest.agenda_days), (1, Some(3)));
    }

    #[test]
    fn quotes() {
        assert_eq!(Query::parse("\"or\"").expr, Some(leaf(text("or"), false)));
        assert_eq!(
            Query::parse("\"f(x)\"").expr,
            Some(leaf(text("f(x)"), false))
        );
        assert_eq!(
            Query::parse("\"i.todo\"").expr,
            Some(leaf(text("i.todo"), false))
        );
        assert_eq!(
            Query::parse(".\"draft copy\"").expr,
            Some(leaf(text("draft copy"), true))
        );
        assert_eq!(
            Query::parse("b.\"My Notebook\"").expr,
            Some(leaf(Condition::Notebook("My Notebook".into()), false))
        );
        assert_eq!(
            Query::parse("\"unclosed phrase").expr,
            Some(leaf(text("unclosed phrase"), false))
        );
        assert_eq!(Query::parse("\"\"").expr, None);
    }

    #[test]
    fn lenient_brackets() {
        assert_eq!(Query::parse("(a b").expr, Query::parse("a b").expr);
        assert_eq!(Query::parse("a ) b").expr, Query::parse("a b").expr);
        assert_eq!(Query::parse("()").expr, None);
        assert_eq!(
            Query::parse("(a b)OR(c)").expr,
            Some(Expr::Or(vec![
                Expr::And(vec![leaf(text("a"), false), leaf(text("b"), false)]),
                leaf(text("c"), false),
            ]))
        );
        assert_eq!(Query::parse("").expr, None);
        assert!(Query::parse("").is_empty());
    }

    #[test]
    fn prefixes() {
        assert_eq!(
            Query::parse("it.done").expr,
            Some(leaf(Condition::StateType(StateType::Done), false))
        );
        assert_eq!(
            Query::parse("it.what").expr,
            Some(leaf(text("it.what"), false))
        );
        assert_eq!(
            Query::parse("ps.b").expr,
            Some(leaf(
                Condition::Priority {
                    priority: "b".into(),
                    set_only: true
                },
                false
            ))
        );
        assert_eq!(
            Query::parse("s.le.2d").expr,
            Some(leaf(
                Condition::Scheduled(Period {
                    raw: "2d".into(),
                    op: Some(CompareOp::Le)
                }),
                false
            ))
        );
        assert_eq!(
            Query::parse("e.today").expr,
            Some(leaf(
                Condition::Event(Period {
                    raw: "today".into(),
                    op: None
                }),
                false
            ))
        );
        assert_eq!(Query::parse("x.").expr, Some(leaf(text("x."), false)));
        assert_eq!(Query::parse(".5").expr, Some(leaf(text(".5"), false)));
        assert_eq!(
            Query::parse("example.com").expr,
            Some(leaf(text("example.com"), false))
        );
    }

    #[test]
    fn directives() {
        let q = Query::parse("d.1w o.d .o.p o.nonsense ad.7");
        assert_eq!(
            q.sort,
            [
                SortKey {
                    field: SortField::Deadline,
                    descending: false
                },
                SortKey {
                    field: SortField::Priority,
                    descending: true
                },
            ]
        );
        assert_eq!(q.agenda_days, Some(7));
        assert_eq!(q.expr.as_ref().map(|e| e.leaves().len()), Some(1));
        assert_eq!(
            Query::parse("phone call \"x y\" .neg i.todo").text_terms(),
            ["phone", "call", "x y"]
        );
    }

    #[test]
    fn periods() {
        let now = NaiveDate::from_ymd_opt(2026, 1, 31)
            .unwrap()
            .and_hms_opt(10, 30, 15)
            .unwrap();
        let today = now.date();
        let p = |s: &str| Period::parse(s).target(now);
        let day = |y, m, d| Target::Day(NaiveDate::from_ymd_opt(y, m, d).unwrap());
        assert_eq!(p("today"), Target::Day(today));
        assert_eq!(p("tmrw"), day(2026, 2, 1));
        assert_eq!(p("yesterday"), day(2026, 1, 30));
        assert_eq!(p("-2w"), day(2026, 1, 17));
        assert_eq!(p("1m"), day(2026, 2, 28), "months clamp");
        assert_eq!(p("+1y"), day(2027, 1, 31));
        assert_eq!(p("2026-03-04"), day(2026, 3, 4));
        assert_eq!(
            p("now"),
            Target::Moment(today.and_hms_opt(10, 30, 0).unwrap())
        );
        assert_eq!(
            p("-2h"),
            Target::Moment(today.and_hms_opt(8, 30, 0).unwrap())
        );
        assert_eq!(p("none"), Target::NoDate);
        assert_eq!(p("overdue"), Target::Overdue);
        assert_eq!(p("eq.none"), Target::Invalid, "specials take no operator");
        assert_eq!(p("junk"), Target::Invalid);
    }
}
