//! Headless agent CLI. No GUI, audio devices, capture, or fixture seeding.
use crate::reader::{KINDS, Reader, Scope};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, NaiveDate, TimeZone};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde_json::Value;
use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
    time::{Duration, Instant},
};
#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Gcf,
    Json,
}
#[derive(Parser)]
#[command(
    name = "worklog",
    bin_name = "worklog",
    version,
    about = "Read your local transcript and computer activity",
    long_about = "Read-only local timeline access. GCF is the default output; --format json is available. Use timeline to read, --after to start after a message or breakpoint, --resume to continue from next_cursor, and --follow for live updates. These commands never open a window or start recording."
)]
struct Cli {
    #[arg(
        long = "database",
        alias = "db",
        global = true,
        help = "SQLite database (also TEAM_UP_SPIKE_DB)"
    )]
    db: Option<PathBuf>,
    #[arg(long, global = true, value_enum, default_value = "gcf")]
    format: Format,
    #[arg(
        long,
        global = true,
        conflicts_with = "format",
        help = "Output JSON instead of GCF"
    )]
    json: bool,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Print a ready-to-save SKILL.md in Markdown; no database is needed.
    Skill,
    /// Show transcript, activity, and annotations; defaults to today.
    #[command(alias = "read")]
    Timeline(TimelineArgs),
    /// Continue a snapshot page or fetch changes since a returned cursor.
    #[command(hide = true)]
    Updates(CursorArgs),
    /// Wait for updates; --stream emits independently framed, resumable batches.
    #[command(hide = true)]
    Watch(WatchArgs),
    /// List days containing saved transcript, activity, or annotations.
    Days {
        #[arg(long,default_value_t=30,value_parser=clap::value_parser!(u32).range(1..=2000))]
        limit: u32,
    },
    /// Read the complete stored OS payload for an activity ID.
    Activity { id: String },
}
#[derive(Args)]
struct TimelineArgs {
    #[command(flatten)]
    query: ReadArgs,
    #[arg(
        long,
        conflicts_with_all=["day","today","from","to","span","after_message","after_breakpoint","kinds"],
        value_name = "TOKEN",
        help = "Continue from the previous response's next_cursor"
    )]
    resume: Option<String>,
    #[arg(
        long,
        conflicts_with = "wait",
        help = "Keep printing new entries; stop with Ctrl-C"
    )]
    follow: bool,
    #[arg(long,requires="resume",conflicts_with="follow",value_name="SECONDS",value_parser=clap::value_parser!(u64).range(1..=86400),help="Wait up to this long for the next batch, then exit")]
    wait: Option<u64>,
    #[arg(long,requires="follow",value_name="SECONDS",value_parser=clap::value_parser!(u64).range(1..=86400),help="Stop following after this many seconds")]
    duration: Option<u64>,
}
#[derive(Args)]
struct ReadArgs {
    #[arg(long="date",alias="day",value_name="DATE",help="Show this calendar date (YYYY-MM-DD, Europe/Berlin)",conflicts_with_all=["today","from","to","span","after_message","after_breakpoint"])]
    day: Option<NaiveDate>,
    #[arg(long,help="Show today (the default)",conflicts_with_all=["day","from","to","span","after_message","after_breakpoint"])]
    today: bool,
    #[arg(long,requires="to",conflicts_with_all=["day","today","span","after_message","after_breakpoint"],help="Inclusive RFC3339 timestamp with timezone")]
    from: Option<DateTime<chrono::FixedOffset>>,
    #[arg(long,requires="from",conflicts_with_all=["day","today","span","after_message","after_breakpoint"],help="Exclusive RFC3339 timestamp with timezone")]
    to: Option<DateTime<chrono::FixedOffset>>,
    #[arg(long="range",alias="span",value_name="ID",help="Show a saved range, such as span:3",conflicts_with_all=["day","today","from","to","after_message","after_breakpoint"])]
    span: Option<String>,
    #[arg(long="after",alias="after-message",value_name="ID",conflicts_with_all=["day","today","from","to","span","after_breakpoint"],help="Start after a message:ID or breakpoint entry:ID")]
    after_message: Option<String>,
    #[arg(long,hide=true,conflicts_with_all=["day","today","from","to","span","after_message"],help="Chronologically after a breakpoint entry:ID")]
    after_breakpoint: Option<String>,
    #[arg(
        long = "include",
        alias = "kinds",
        value_name = "TYPES",
        help = "Only show these comma-separated entry types",
        value_delimiter = ',',
        default_value = "transcript,activity,note,breakpoint,span,comment"
    )]
    kinds: Vec<String>,
    #[arg(long,default_value_t=200,value_parser=clap::value_parser!(u32).range(1..=2000))]
    limit: u32,
}
#[derive(Args)]
struct CursorArgs {
    #[arg(long)]
    cursor: String,
    #[arg(long,default_value_t=200,value_parser=clap::value_parser!(u32).range(1..=2000))]
    limit: u32,
}
#[derive(Args)]
struct WatchArgs {
    #[command(flatten)]
    read: CursorArgs,
    #[arg(
        long,
        help = "Emit multiple documents, each with a continuation cursor, and flush after every batch"
    )]
    stream: bool,
    #[arg(long,default_value_t=30,value_parser=clap::value_parser!(u64).range(0..=86400),help="Seconds to wait (default 30). Zero means unlimited and requires --stream")]
    timeout: u64,
}
pub fn database_path(explicit: Option<PathBuf>) -> Result<PathBuf> {
    explicit
        .or_else(|| std::env::var_os("TEAM_UP_SPIKE_DB").map(PathBuf::from))
        .map(Ok)
        .unwrap_or_else(|| {
            Ok(
                PathBuf::from(std::env::var_os("HOME").context("HOME is unavailable")?)
                    .join("Library/Application Support/worklog/worklog.sqlite"),
            )
        })
}
fn day_range(day: NaiveDate) -> Result<(i64, i64)> {
    let start = chrono_tz::Europe::Berlin
        .from_local_datetime(&day.and_hms_opt(0, 0, 0).context("Invalid day")?)
        .single()
        .context("Ambiguous day boundary")?
        .timestamp_millis();
    let next = day.succ_opt().context("Date is out of range")?;
    let end = chrono_tz::Europe::Berlin
        .from_local_datetime(&next.and_hms_opt(0, 0, 0).context("Invalid day")?)
        .single()
        .context("Ambiguous day boundary")?
        .timestamp_millis();
    Ok((start, end))
}
fn scope(args: ReadArgs, reader: &Reader) -> Result<Scope> {
    if args
        .kinds
        .iter()
        .any(|kind| !KINDS.contains(&kind.as_str()))
    {
        bail!("Unknown kind. Choose from {}", KINDS.join(","));
    }
    let mut after = None;
    let (from, to) = if let Some(id) = args.after_message {
        let anchor = reader.anchor(&id, id.starts_with("entry:"))?;
        let start = anchor.0;
        after = Some(anchor);
        (start, i64::MAX)
    } else if let Some(id) = args.after_breakpoint {
        let anchor = reader.anchor(&id, true)?;
        let start = anchor.0;
        after = Some(anchor);
        (start, i64::MAX)
    } else if let Some(id) = args.span {
        reader.span(&id)?
    } else if let Some(from) = args.from {
        (
            from.timestamp_millis(),
            args.to.context("Missing --to")?.timestamp_millis(),
        )
    } else {
        day_range(args.day.unwrap_or_else(|| {
            chrono::Utc::now()
                .with_timezone(&chrono_tz::Europe::Berlin)
                .date_naive()
        }))?
    };
    let mut kinds = args.kinds;
    kinds.sort();
    kinds.dedup();
    let scope = Scope {
        from,
        to,
        kinds,
        after,
    };
    scope.validate()?;
    Ok(scope)
}
fn output(value: &Value, format: Format, stream: bool, out: &mut impl Write) -> Result<()> {
    let text = match format {
        Format::Gcf => gcf::encode_generic(value)
            .map_err(anyhow::Error::msg)
            .context(
                "GCF encoding failed; --format json supports values outside GCF's integer domain",
            )?,
        Format::Json => {
            if stream {
                serde_json::to_string(value)?
            } else {
                serde_json::to_string_pretty(value)?
            }
        }
    };
    writeln!(out, "{}", text.trim_end())?;
    if stream && matches!(format, Format::Gcf) {
        writeln!(out)?;
    }
    out.flush()?;
    Ok(())
}
fn watch(
    reader: &Reader,
    mut cursor: String,
    limit: usize,
    stream: bool,
    timeout: Option<u64>,
    format: Format,
    out: &mut impl Write,
) -> Result<()> {
    let deadline = timeout.map(|seconds| Instant::now() + Duration::from_secs(seconds));
    loop {
        let page = reader.next(&cursor, limit)?;
        let expired = deadline.is_some_and(|deadline| Instant::now() >= deadline);
        let ready = !page.events.is_empty() || page.has_more || page.mode == "snapshot";
        cursor = page.next_cursor.clone();
        let more = page.has_more;
        if ready || expired {
            output(&serde_json::to_value(page)?, format, stream, out)?;
            if !stream || expired {
                break;
            }
        }
        if !more {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    Ok(())
}
fn explicit_scope(args: &ReadArgs) -> bool {
    args.day.is_some()
        || args.today
        || args.from.is_some()
        || args.to.is_some()
        || args.span.is_some()
        || args.after_message.is_some()
        || args.after_breakpoint.is_some()
        || args.kinds.iter().map(String::as_str).collect::<Vec<_>>() != KINDS
}
pub fn run(args: impl IntoIterator<Item = OsString>) -> Result<bool> {
    let args: Vec<_> = args.into_iter().collect();
    if args.len() == 1 {
        return Ok(false);
    }
    let cli = Cli::parse_from(args);
    if matches!(&cli.command, Command::Skill) {
        io::stdout()
            .lock()
            .write_all(include_bytes!("../../../skills/worklog/SKILL.md"))?;
        return Ok(true);
    }
    let format = if cli.json { Format::Json } else { cli.format };
    let path = database_path(cli.db)?;
    let reader = Reader::open(&path)?;
    let stdout = io::stdout();
    let mut out = stdout.lock();
    match cli.command {
        Command::Skill => unreachable!("Skill output returns before opening the database"),
        Command::Timeline(args) => {
            let limit = args.query.limit as usize;
            let cursor = if let Some(token) = args.resume {
                if explicit_scope(&args.query) {
                    bail!(
                        "A resume token already includes its date/range and entry types. Start a new timeline query to change them."
                    );
                }
                token
            } else {
                let page = reader.snapshot(scope(args.query, &reader)?, limit)?;
                let cursor = page.next_cursor.clone();
                output(&serde_json::to_value(page)?, format, args.follow, &mut out)?;
                if !args.follow {
                    return Ok(true);
                }
                cursor
            };
            if args.follow {
                watch(
                    &reader,
                    cursor,
                    limit,
                    true,
                    args.duration,
                    format,
                    &mut out,
                )?;
            } else if let Some(wait) = args.wait {
                watch(&reader, cursor, limit, false, Some(wait), format, &mut out)?;
            } else {
                output(
                    &serde_json::to_value(reader.next(&cursor, limit)?)?,
                    format,
                    false,
                    &mut out,
                )?;
            }
        }
        Command::Updates(args) => output(
            &serde_json::to_value(reader.next(&args.cursor, args.limit as usize)?)?,
            format,
            false,
            &mut out,
        )?,
        Command::Watch(args) => {
            if args.timeout == 0 && !args.stream {
                bail!("--timeout 0 requires --stream; ordinary watch calls must be bounded");
            }
            watch(
                &reader,
                args.read.cursor,
                args.read.limit as usize,
                args.stream,
                if args.timeout == 0 {
                    None
                } else {
                    Some(args.timeout)
                },
                format,
                &mut out,
            )?;
        }
        Command::Days { limit } => output(&reader.days(limit as usize)?, format, false, &mut out)?,
        Command::Activity { id } => output(&reader.activity(&id)?, format, false, &mut out)?,
    }
    Ok(true)
}
