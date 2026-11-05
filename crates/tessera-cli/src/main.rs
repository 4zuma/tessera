//! `tessera`: command-line front end for the Tessera core.
//!
//! Used to benchmark the scanner against the targets in the architecture
//! doc (non-admin full C: scan < 60 s, < 300 MB, 0 % idle CPU) and to
//! exercise the core without the GUI.
//!
//! ```text
//! tessera scan  <PATH> [--mode auto|walk|mft] [--threads N] [--top N] [--json]
//! tessera bench <PATH> [--mode auto|walk|mft] [--threads N] [--runs N] [--idle SECS]
//! ```

mod sys;

use std::collections::BinaryHeap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tessera_core::scan::{self, ScanMode, ScanOptions, ScanProgress, ScanSummary};
use tessera_core::{NodeId, Tree};

const USAGE: &str = "\
usage:
  tessera scan  <PATH> [--mode auto|walk|mft] [--threads N] [--top N] [--json]
  tessera bench <PATH> [--mode auto|walk|mft] [--threads N] [--runs N] [--idle SECS]

scan   scans PATH once and prints totals, memory and the largest items
bench  scans PATH several times, then keeps the last tree and measures idle
       CPU for SECS seconds; prints one JSON object
";

struct Args {
    command: String,
    path: PathBuf,
    mode: ScanMode,
    threads: usize,
    top: usize,
    json: bool,
    runs: usize,
    idle_secs: u64,
}

fn parse_args() -> Result<Args, String> {
    let mut it = std::env::args_os().skip(1);
    let command = it
        .next()
        .ok_or("missing command")?
        .into_string()
        .map_err(|_| "bad command")?;
    let mut args = Args {
        command,
        path: PathBuf::new(),
        mode: ScanMode::Auto,
        threads: 0,
        top: 10,
        json: false,
        runs: 3,
        idle_secs: 10,
    };
    let mut path = None;
    while let Some(a) = it.next() {
        let s = a.to_string_lossy().into_owned();
        let mut value = |name: &str| -> Result<String, String> {
            it.next()
                .map(|v| v.to_string_lossy().into_owned())
                .ok_or(format!("{name} needs a value"))
        };
        match s.as_str() {
            "--mode" => {
                args.mode = match value("--mode")?.as_str() {
                    "auto" => ScanMode::Auto,
                    "walk" => ScanMode::Walk,
                    "mft" => ScanMode::Mft,
                    other => return Err(format!("unknown mode {other}")),
                }
            }
            "--threads" => args.threads = num(&value("--threads")?)?,
            "--top" => args.top = num(&value("--top")?)?,
            "--runs" => args.runs = num::<usize>(&value("--runs")?)?.max(1),
            "--idle" => args.idle_secs = num(&value("--idle")?)?,
            "--json" => args.json = true,
            "-h" | "--help" => return Err(String::new()),
            _ if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ if path.is_none() => path = Some(PathBuf::from(a)),
            _ => return Err(format!("unexpected argument {s}")),
        }
    }
    args.path = path.ok_or("missing PATH")?;
    Ok(args)
}

fn num<T: std::str::FromStr>(s: &str) -> Result<T, String> {
    s.parse().map_err(|_| format!("not a number: {s}"))
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("error: {e}\n");
            }
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let result = match args.command.as_str() {
        "scan" => cmd_scan(&args),
        "bench" => cmd_bench(&args),
        other => Err(format!("unknown command {other}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Runs one scan on a worker thread while the main thread prints progress
/// to stderr every 500 ms (only while the scan runs).
fn run_scan(args: &Args, show_progress: bool) -> Result<(Tree, ScanSummary), String> {
    let tree = Tree::for_path(&args.path).into_shared();
    let progress = Arc::new(ScanProgress::new());
    let opts = ScanOptions {
        mode: args.mode,
        threads: args.threads,
    };
    let worker = {
        let (tree, progress, path) = (tree.clone(), progress.clone(), args.path.clone());
        std::thread::spawn(move || scan::scan(&path, &opts, &tree, &progress))
    };
    let start = Instant::now();
    while !worker.is_finished() {
        std::thread::park_timeout(Duration::from_millis(500));
        if show_progress && !worker.is_finished() {
            let p = progress.snapshot();
            eprint!(
                "\r  {:>6.1}s  {:>10} files  {:>9} dirs  {:>10}",
                start.elapsed().as_secs_f64(),
                p.files,
                p.dirs,
                human(p.bytes)
            );
        }
    }
    if show_progress {
        eprint!("\r{:72}\r", "");
    }
    let summary = worker
        .join()
        .map_err(|_| "scanner thread panicked".to_string())?
        .map_err(|e| e.to_string())?;
    let tree = Arc::try_unwrap(tree)
        .map_err(|_| "tree still shared".to_string())?
        .into_inner()
        .map_err(|_| "tree lock poisoned".to_string())?;
    Ok((tree, summary))
}

fn cmd_scan(args: &Args) -> Result<(), String> {
    let (tree, summary) = run_scan(args, !args.json)?;
    let mem = sys::memory();
    let tmem = tree.memory();
    let root = tree.node(NodeId::ROOT);

    let mut top_dirs: Vec<NodeId> = tree.children(NodeId::ROOT).collect();
    top_dirs.sort_by_key(|&c| std::cmp::Reverse(tree.node(c).size));
    top_dirs.truncate(args.top);
    let top_files = largest_files(&tree, args.top);

    if args.json {
        let mut j = Json::new();
        j.str("path", &args.path.to_string_lossy());
        j.str("mode", &format!("{:?}", summary.mode));
        j.num("elapsed_ms", summary.elapsed.as_millis() as u64);
        j.num("files", summary.files);
        j.num("dirs", summary.dirs);
        j.num("inaccessible", summary.inaccessible);
        j.num("nodes", tree.len() as u64);
        j.num("size_on_disk", root.size);
        j.num("logical_size", root.logical);
        j.num("tree_bytes", tmem.total() as u64);
        j.num("working_set", mem.working_set);
        j.num("peak_working_set", mem.peak_working_set);
        j.num("private_bytes", mem.private_bytes);
        println!("{}", j.finish());
        return Ok(());
    }

    println!("{}", args.path.display());
    println!(
        "  scanner        {:?}{}",
        summary.mode,
        if scan::is_elevated() {
            " (elevated)"
        } else {
            ""
        }
    );
    println!("  elapsed        {:.2} s", summary.elapsed.as_secs_f64());
    println!(
        "  entries        {} files, {} dirs, {} inaccessible",
        summary.files, summary.dirs, summary.inaccessible
    );
    println!(
        "  total          {} on disk, {} logical",
        human(root.size),
        human(root.logical)
    );
    if let Some(v) = &summary.volume {
        println!(
            "  volume         {} {} total, {} free",
            v.file_system,
            human(v.total_bytes),
            human(v.free_bytes)
        );
    }
    println!(
        "  tree memory    {} ({} nodes, {} names)",
        human(tmem.total() as u64),
        human(tmem.nodes as u64),
        human(tmem.names as u64)
    );
    println!(
        "  process memory {} working set, {} peak, {} private",
        human(mem.working_set),
        human(mem.peak_working_set),
        human(mem.private_bytes)
    );
    if !top_dirs.is_empty() {
        println!("\nlargest entries in {}", args.path.display());
        for c in top_dirs {
            let n = tree.node(c);
            println!(
                "  {:>10}  {}{}",
                human(n.size),
                tree.name_lossy(c),
                if n.is_dir() { "\\" } else { "" }
            );
        }
    }
    if !top_files.is_empty() {
        println!("\nlargest files");
        for f in top_files {
            println!("  {:>10}  {}", human(tree.node(f).size), tree.path_lossy(f));
        }
    }
    Ok(())
}

fn cmd_bench(args: &Args) -> Result<(), String> {
    let mut times = Vec::with_capacity(args.runs);
    let mut last: Option<(Tree, ScanSummary)> = None;
    for run in 0..args.runs {
        // Drop the previous tree first so runs don't stack up memory.
        drop(last.take());
        let (tree, summary) = run_scan(args, false)?;
        eprintln!(
            "run {}: {:.2} s, {} nodes",
            run + 1,
            summary.elapsed.as_secs_f64(),
            tree.len()
        );
        times.push(summary.elapsed);
        last = Some((tree, summary));
    }
    let (tree, summary) = last.expect("at least one run");
    let after_scan = sys::memory();

    // Idle: keep the tree alive and do nothing; any CPU time is overhead.
    let cpu_before = sys::cpu_time();
    let idle_start = Instant::now();
    std::thread::sleep(Duration::from_secs(args.idle_secs));
    let idle_wall = idle_start.elapsed();
    let idle_cpu = sys::cpu_time().saturating_sub(cpu_before);

    times.sort();
    let median = times[times.len() / 2];
    let mut j = Json::new();
    j.str("path", &args.path.to_string_lossy());
    j.str("mode", &format!("{:?}", summary.mode));
    j.bool("elevated", scan::is_elevated());
    j.num("runs", args.runs as u64);
    j.num("min_ms", times[0].as_millis() as u64);
    j.num("median_ms", median.as_millis() as u64);
    j.num("max_ms", times[times.len() - 1].as_millis() as u64);
    j.num("files", summary.files);
    j.num("dirs", summary.dirs);
    j.num("inaccessible", summary.inaccessible);
    j.num("nodes", tree.len() as u64);
    j.num("size_on_disk", tree.node(NodeId::ROOT).size);
    j.num("tree_bytes", tree.memory().total() as u64);
    j.num("working_set_after_scan", after_scan.working_set);
    j.num("private_bytes_after_scan", after_scan.private_bytes);
    j.num("peak_working_set", after_scan.peak_working_set);
    j.num("idle_secs", idle_wall.as_secs());
    j.float(
        "idle_cpu_percent_of_one_core",
        100.0 * idle_cpu.as_secs_f64() / idle_wall.as_secs_f64(),
    );
    println!("{}", j.finish());
    drop(tree);
    Ok(())
}

/// The `n` largest files (by size on disk) in the whole tree.
fn largest_files(tree: &Tree, n: usize) -> Vec<NodeId> {
    if n == 0 {
        return Vec::new();
    }
    let mut heap = BinaryHeap::with_capacity(n + 1);
    for i in 1..tree.len() as u32 {
        let id = NodeId(i);
        let node = tree.node(id);
        if node.is_dir() || node.size == 0 {
            continue;
        }
        if heap.len() < n {
            heap.push(std::cmp::Reverse((node.size, id)));
        } else if heap.peek().is_some_and(|m| m.0.0 < node.size) {
            heap.pop();
            heap.push(std::cmp::Reverse((node.size, id)));
        }
    }
    let mut v: Vec<(u64, NodeId)> = heap.into_iter().map(|r| r.0).collect();
    v.retain(|&(_, id)| !tree.is_deleted(id));
    v.sort_by(|a, b| b.cmp(a));
    v.into_iter().map(|(_, id)| id).collect()
}

fn human(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// A tiny JSON object writer (keeps the CLI dependency-free).
struct Json(String);

impl Json {
    fn new() -> Self {
        Json(String::from("{"))
    }
    fn key(&mut self, k: &str) {
        if self.0.len() > 1 {
            self.0.push(',');
        }
        self.0.push('"');
        self.0.push_str(k);
        self.0.push_str("\":");
    }
    fn str(&mut self, k: &str, v: &str) {
        self.key(k);
        self.0.push('"');
        for c in v.chars() {
            match c {
                '"' => self.0.push_str("\\\""),
                '\\' => self.0.push_str("\\\\"),
                c if (c as u32) < 0x20 => self.0.push_str(&format!("\\u{:04x}", c as u32)),
                c => self.0.push(c),
            }
        }
        self.0.push('"');
    }
    fn num(&mut self, k: &str, v: u64) {
        self.key(k);
        self.0.push_str(&v.to_string());
    }
    fn float(&mut self, k: &str, v: f64) {
        self.key(k);
        self.0.push_str(&format!("{v:.3}"));
    }
    fn bool(&mut self, k: &str, v: bool) {
        self.key(k);
        self.0.push_str(if v { "true" } else { "false" });
    }
    fn finish(mut self) -> String {
        self.0.push('}');
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_sizes() {
        assert_eq!(human(0), "0 B");
        assert_eq!(human(1023), "1023 B");
        assert_eq!(human(1536), "1.5 KB");
        assert_eq!(human(5 * 1024 * 1024 * 1024), "5.0 GB");
    }

    #[test]
    fn json_escapes() {
        let mut j = Json::new();
        j.str("p", "C:\\a \"b\"\n");
        j.num("n", 3);
        j.bool("b", true);
        assert_eq!(j.finish(), r#"{"p":"C:\\a \"b\"\u000a","n":3,"b":true}"#);
    }

    #[test]
    fn largest_files_picks_biggest() {
        let mut t = Tree::new(b"X:\\");
        let mut b = tessera_core::EntryBatch::new();
        for (i, s) in [5u64, 50, 7, 900, 3].iter().enumerate() {
            b.push_wtf8(format!("f{i}").as_bytes(), *s, *s, Default::default(), 0);
        }
        t.add_children(NodeId::ROOT, &b);
        let top: Vec<String> = largest_files(&t, 2)
            .into_iter()
            .map(|id| t.name_lossy(id))
            .collect();
        assert_eq!(top, ["f3", "f1"]);
    }
}
