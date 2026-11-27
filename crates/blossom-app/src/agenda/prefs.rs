//! The agenda's settings (`[agenda]` in the config): the first day of the
//! week, working hours and up to two extra time zones.

use std::sync::LazyLock;

use blossom_vault::config::AgendaConfig;
use blossom_vault::config::edit::{Change, Value};
use chrono::{Datelike, NaiveDate, Weekday};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

const TABLE: Option<&str> = Some("agenda");

/// How many extra time zones the week view shows at most.
pub const MAX_ZONES: usize = 2;

/// The first day of the week.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WeekStart {
    /// The system's choice (Monday when it can't be told).
    #[default]
    System,
    /// Monday.
    Monday,
    /// Sunday.
    Sunday,
    /// Saturday.
    Saturday,
}

impl WeekStart {
    /// Every choice, as the Settings panel lists them.
    pub const ALL: [WeekStart; 4] = [
        WeekStart::System,
        WeekStart::Monday,
        WeekStart::Sunday,
        WeekStart::Saturday,
    ];

    /// The config name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            WeekStart::System => "system",
            WeekStart::Monday => "monday",
            WeekStart::Sunday => "sunday",
            WeekStart::Saturday => "saturday",
        }
    }

    /// The day it stands for.
    #[must_use]
    pub fn weekday(self) -> Weekday {
        match self {
            WeekStart::System => *SYSTEM,
            WeekStart::Monday => Weekday::Mon,
            WeekStart::Sunday => Weekday::Sun,
            WeekStart::Saturday => Weekday::Sat,
        }
    }
}

impl std::fmt::Display for WeekStart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WeekStart::System => write!(f, "Follow the system ({})", day_name(*SYSTEM)),
            WeekStart::Monday => f.write_str("Monday"),
            WeekStart::Sunday => f.write_str("Sunday"),
            WeekStart::Saturday => f.write_str("Saturday"),
        }
    }
}

/// `Monday` for `Weekday::Mon`.
#[must_use]
pub fn day_name(d: Weekday) -> &'static str {
    match d {
        Weekday::Mon => "Monday",
        Weekday::Tue => "Tuesday",
        Weekday::Wed => "Wednesday",
        Weekday::Thu => "Thursday",
        Weekday::Fri => "Friday",
        Weekday::Sat => "Saturday",
        Weekday::Sun => "Sunday",
    }
}

/// The weekday `n` days after Sunday (`n` taken modulo 7).
fn from_sunday(n: u32) -> Weekday {
    match n % 7 {
        0 => Weekday::Sun,
        1 => Weekday::Mon,
        2 => Weekday::Tue,
        3 => Weekday::Wed,
        4 => Weekday::Thu,
        5 => Weekday::Fri,
        _ => Weekday::Sat,
    }
}

/// The system's first day of the week, read once.
static SYSTEM: LazyLock<Weekday> = LazyLock::new(|| system_week_start().unwrap_or(Weekday::Mon));

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(cmd).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The system's first day of the week: on macOS the `AppleFirstWeekday`
/// default, elsewhere the locale's `first_weekday` (`LC_TIME`). `None`
/// when it can't be told, or the locale is C/POSIX, which says nothing.
fn system_week_start() -> Option<Weekday> {
    if cfg!(target_os = "macos") {
        // `{ gregorian = 2; }`, where 1 is Sunday.
        let text = run("defaults", &["read", "-g", "AppleFirstWeekday"])?;
        return parse_apple(&text);
    }
    let lc_time = std::env::var("LC_ALL")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var("LC_TIME").ok().filter(|v| !v.is_empty()))
        .or_else(|| std::env::var("LANG").ok())
        .unwrap_or_default();
    if lc_time.is_empty() || lc_time.starts_with('C') || lc_time.starts_with("POSIX") {
        return None;
    }
    parse_locale(&run("locale", &["week-1stday", "first_weekday"])?)
}

/// `locale week-1stday first_weekday`'s output: a date and a 1-based day
/// counted from it (`19971130` is a Sunday; `2` makes Monday).
fn parse_locale(text: &str) -> Option<Weekday> {
    let mut lines = text.lines();
    let base = NaiveDate::parse_from_str(lines.next()?.trim(), "%Y%m%d").ok()?;
    let n: u32 = lines.next()?.trim().parse().ok()?;
    Some(from_sunday(
        base.weekday().num_days_from_sunday() + n.checked_sub(1)?,
    ))
}

fn parse_apple(text: &str) -> Option<Weekday> {
    let n: u32 = text
        .split('=')
        .nth(1)?
        .trim_matches(|c: char| !c.is_ascii_digit())
        .parse()
        .ok()?;
    Some(from_sunday(n.checked_sub(1)?))
}

/// The agenda's settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgendaPrefs {
    /// The first day of the week.
    pub week_start: WeekStart,
    /// Working hours, start and end hour (`end` up to 24).
    pub work: (u32, u32),
    /// Extra time zones, at most [`MAX_ZONES`].
    pub zones: Vec<Tz>,
}

impl Default for AgendaPrefs {
    fn default() -> AgendaPrefs {
        AgendaPrefs {
            week_start: WeekStart::System,
            work: (8, 20),
            zones: Vec::new(),
        }
    }
}

/// `"08:00-20:00"` as (8, 20): whole hours, start before end.
fn parse_hours(text: &str) -> Option<(u32, u32)> {
    let (from, to) = text.split_once('-')?;
    let hour = |part: &str| -> Option<u32> {
        let part = part.trim();
        let (hours, minutes) = part.split_once(':').unwrap_or((part, "00"));
        (minutes.trim() == "00").then_some(())?;
        hours.trim().parse().ok()
    };
    let (start, end) = (hour(from)?, hour(to)?);
    (start < end && end <= 24).then_some((start, end))
}

impl AgendaPrefs {
    /// The settings in `config`; a value that isn't valid gives the
    /// default and a message saying so.
    #[must_use]
    pub fn from_config(config: &AgendaConfig, problems: &mut Vec<String>) -> AgendaPrefs {
        let d = AgendaPrefs::default();
        let week_start = match config.week_start.as_deref() {
            None => d.week_start,
            Some(name) => WeekStart::ALL
                .into_iter()
                .find(|w| w.name().eq_ignore_ascii_case(name.trim()))
                .unwrap_or_else(|| {
                    problems.push(format!(
                        "week_start = \"{name}\" isn't one of system, monday, sunday or saturday; \
                         following the system"
                    ));
                    d.week_start
                }),
        };
        let work = match config.working_hours.as_deref() {
            None => d.work,
            Some(s) => parse_hours(s).unwrap_or_else(|| {
                problems.push(format!(
                    "working_hours = \"{s}\" should be whole hours like \"08:00-20:00\"; \
                     using 08:00-20:00"
                ));
                d.work
            }),
        };
        let mut zones = Vec::new();
        for name in &config.time_zones {
            match name.trim().parse::<Tz>() {
                Ok(tz) if zones.len() < MAX_ZONES && !zones.contains(&tz) => zones.push(tz),
                Ok(_) => {}
                Err(_) => problems.push(format!(
                    "time_zones: \"{name}\" isn't a time zone name like \"Asia/Tokyo\""
                )),
            }
        }
        AgendaPrefs {
            week_start,
            work,
            zones,
        }
    }

    /// The config changes from `old` to these settings.
    #[must_use]
    pub fn changes_from(&self, old: &AgendaPrefs) -> Vec<Change> {
        let mut out = Vec::new();
        if self.week_start != old.week_start {
            out.push(Change::set(TABLE, "week_start", self.week_start.name()));
        }
        if self.work != old.work {
            out.push(Change::set(
                TABLE,
                "working_hours",
                format!("{:02}:00-{:02}:00", self.work.0, self.work.1),
            ));
        }
        if self.zones != old.zones {
            let names = self.zones.iter().map(|z| z.name().to_string()).collect();
            out.push(Change::set(TABLE, "time_zones", Value::List(names)));
        }
        out
    }

    /// The first day of the week.
    #[must_use]
    pub fn first_day(&self) -> Weekday {
        self.week_start.weekday()
    }

    /// The first day of the week holding `date`.
    #[must_use]
    pub fn week_of(&self, date: NaiveDate) -> NaiveDate {
        let back = (date.weekday().num_days_from_sunday() + 7
            - self.first_day().num_days_from_sunday())
            % 7;
        date - chrono::Days::new(u64::from(back))
    }
}

/// Every time zone name, for the Settings panel.
#[must_use]
pub fn zone_names() -> Vec<String> {
    chrono_tz::TZ_VARIANTS
        .iter()
        .map(|z| z.name().to_string())
        .filter(|n| n.contains('/') || n == "UTC")
        .filter(|n| !n.starts_with("Etc/") && !n.starts_with("SystemV/"))
        .collect()
}

/// `New York` for `America/New_York`.
#[must_use]
pub fn city(tz: Tz) -> String {
    let name = tz.name();
    name.rsplit('/').next().unwrap_or(name).replace('_', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_and_apple_week_starts() {
        assert_eq!(parse_locale("19971130\n1\n"), Some(Weekday::Sun));
        assert_eq!(parse_locale("19971130\n2\n"), Some(Weekday::Mon));
        assert_eq!(parse_locale("19971201\n1\n"), Some(Weekday::Mon));
        assert_eq!(parse_locale("nonsense"), None);
        assert_eq!(
            parse_apple("{\n    gregorian = 2;\n}\n"),
            Some(Weekday::Mon)
        );
        assert_eq!(parse_apple("{ gregorian = 7; }"), Some(Weekday::Sat));
    }

    #[test]
    fn config_values() {
        let mut problems = Vec::new();
        let c = AgendaConfig {
            week_start: Some("Sunday".into()),
            working_hours: Some("09:00-17:00".into()),
            time_zones: vec![
                "Asia/Tokyo".into(),
                "Mars/Base".into(),
                "UTC".into(),
                "Europe/Kyiv".into(),
            ],
        };
        let p = AgendaPrefs::from_config(&c, &mut problems);
        assert_eq!(p.week_start, WeekStart::Sunday);
        assert_eq!(p.work, (9, 17));
        assert_eq!(p.zones, [chrono_tz::Asia::Tokyo, chrono_tz::UTC]);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(parse_hours("20:00-08:00"), None);
        assert_eq!(parse_hours("08:30-20:00"), None);
        assert_eq!(parse_hours("0-24"), Some((0, 24)));
        // A week from Sunday 22 Nov 2026 holds Thursday the 26th.
        let thu = NaiveDate::from_ymd_opt(2026, 11, 26).unwrap();
        assert_eq!(
            p.week_of(thu),
            NaiveDate::from_ymd_opt(2026, 11, 22).unwrap()
        );
        let mon = AgendaPrefs {
            week_start: WeekStart::Monday,
            ..p.clone()
        };
        assert_eq!(
            mon.week_of(thu),
            NaiveDate::from_ymd_opt(2026, 11, 23).unwrap()
        );
        let changes = mon.changes_from(&p);
        assert_eq!(changes.len(), 1);
        assert_eq!(city(chrono_tz::America::New_York), "New York");
        assert!(zone_names().contains(&"Asia/Tokyo".to_string()));
    }
}
