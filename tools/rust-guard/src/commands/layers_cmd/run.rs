//! `rust-guard layers` orchestration: per configured crate, build the
//! module map, collect production references, compare against the
//! layering, and ratchet the baseline.

use crate::commands::layers_cmd::config::{self, pair_key};
use crate::commands::layers_cmd::graph;
use crate::commands::layers_cmd::refs;
use crate::common::finding::{exit_code, Report};
use crate::common::gitfs::GitFs;
use std::collections::BTreeMap;
use std::path::PathBuf;

const HEADER: &str = "# rust-guard layers ratchet baseline (machine-managed; do not edit:
# `rust-guard layers --update-baseline` rewrites this file).
# `\"A -> B\" = files`: production files in layer A with at least one
# backward reference into layer B. Counts may only go down.
";

/// CLI inputs of the `layers` subcommand.
pub(crate) struct LayersArgs {
    pub(crate) repo: PathBuf,
    pub(crate) config: Option<String>,
    pub(crate) update_baseline: bool,
}

/// (from_layer, to_layer) -> file -> first backward-reference line.
type Pairs = BTreeMap<(String, String), BTreeMap<String, usize>>;

/// Runs the gate; returns the process exit code.
pub(crate) fn run(args: LayersArgs) -> u8 {
    match execute(&args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("rust-guard: error: {e}");
            1
        }
    }
}

fn execute(args: &LayersArgs) -> Result<u8, String> {
    let cfg_path = args.config.as_deref().unwrap_or(config::DEFAULT_CONFIG);
    let crates = config::load_config(cfg_path)?;
    let mut baseline = config::load_baseline(config::DEFAULT_BASELINE)?;
    let fs = GitFs::open(None, &args.repo)?;
    let mut report = Report::new();
    let mut measured: BTreeMap<String, Pairs> = BTreeMap::new();

    for crate_cfg in &crates {
        for msg in config::validate_crate(crate_cfg, baseline.crates.get(&crate_cfg.name)) {
            report.add(cfg_path, 1, "LAYER_BASELINE_BAD", msg);
        }
        let cfg = crate_cfg;
        let g = graph::build(&fs, &args.repo, &cfg.name)?;
        for (file, logical) in &g.files {
            if logical.is_empty() {
                continue;
            }
            if config::layer_of(logical, &cfg.layers, &cfg.modules).is_none() {
                report.add(
                    file,
                    1,
                    "LAYER_UNASSIGNED",
                    format!(
                        "module `{}` is not covered by any configured layer; add it to layers.toml",
                        logical.join("::")
                    ),
                );
            }
        }
        let mut pairs: Pairs = BTreeMap::new();
        for (file, logical) in &g.files {
            let own = config::layer_of(logical, &cfg.layers, &cfg.modules);
            let Some((from_name, from_idx)) = own else {
                // Exempt crate root, or unassigned (already reported).
                continue;
            };
            let source = fs.read(file).ok_or_else(|| format!("{file}: unreadable"))?;
            let file_refs =
                refs::collect(&source, logical, &g).map_err(|e| format!("{file}: {e}"))?;
            for (target, line) in &file_refs.targets {
                let Some((to_name, to_idx)) = config::layer_of(target, &cfg.layers, &cfg.modules)
                else {
                    // The target's own file carries the LAYER_UNASSIGNED finding.
                    continue;
                };
                // Layers are ordered lowest first; a module may refer
                // to its own layer and lower ones only, so a reference
                // to a HIGHER layer is the violation.
                if to_idx > from_idx {
                    pairs
                        .entry((from_name.clone(), to_name))
                        .or_default()
                        .entry(file.clone())
                        .and_modify(|l| {
                            if *line < *l {
                                *l = *line;
                            }
                        })
                        .or_insert(*line);
                }
            }
        }
        measured.insert(cfg.name.clone(), pairs);
    }

    compare_baseline(cfg_path, &baseline, &measured, &mut report);

    if args.update_baseline {
        refuse_growth(&measured, &baseline)?;
        baseline.crates = written_baseline(&measured);
        std::fs::write(
            config::DEFAULT_BASELINE,
            format!(
                "{HEADER}{}",
                toml::to_string(&baseline).map_err(|e| e.to_string())?
            ),
        )
        .map_err(|e| format!("{}: {e}", config::DEFAULT_BASELINE))?;
        eprintln!(
            "rust-guard: note: baseline written to {}",
            config::DEFAULT_BASELINE
        );
        report.retain(|f| f.code != "STALE_BASELINE");
        report.print();
        return Ok(exit_code(&report));
    }
    report.print();
    Ok(exit_code(&report))
}

/// Adds LAYER_BACKWARD / STALE_BASELINE findings from the ratchet
/// comparison.
fn compare_baseline(
    cfg_path: &str,
    baseline: &config::BaselineFile,
    measured: &BTreeMap<String, Pairs>,
    report: &mut Report,
) {
    for (crate_name, pairs) in measured {
        let section = baseline.crates.get(crate_name);
        for ((a, b), files) in pairs {
            let Some(n) = section.and_then(|m| m.get(&pair_key(a, b))) else {
                for (file, line) in files {
                    report.add(
                        file,
                        *line,
                        "LAYER_BACKWARD",
                        format!("{a} -> {b} (new backward pair; allowed direction is {b} -> {a})"),
                    );
                }
                continue;
            };
            if files.len() as u64 > *n {
                for (file, line) in files {
                    report.add(
                        file,
                        *line,
                        "LAYER_BACKWARD",
                        format!(
                            "{a} -> {b} (baseline {n} exceeded by {} file(s); allowed direction is {b} -> {a})",
                            files.len()
                        ),
                    );
                }
            }
        }
        let Some(section) = section else {
            continue;
        };
        for (key, n) in section {
            let Some((a, b)) = config::parse_pair(key) else {
                continue;
            };
            if !pairs.contains_key(&(a, b)) {
                report.add(
                    cfg_path,
                    1,
                    "STALE_BASELINE",
                    format!(
                        "baseline entry `{key}` = {n} matches no backward reference; prune it (--update-baseline lowers or removes entries)"
                    ),
                );
            }
        }
    }
}

/// Refuses an update when a measured pair is new or has grown.
fn refuse_growth(
    measured: &BTreeMap<String, Pairs>,
    baseline: &config::BaselineFile,
) -> Result<(), String> {
    for (crate_name, pairs) in measured {
        let section = baseline.crates.get(crate_name);
        for ((a, b), files) in pairs {
            match section.and_then(|m| m.get(&pair_key(a, b))) {
                None => {
                    return Err(format!(
                        "--update-baseline refused for crate `{crate_name}`: new backward pair `{a} -> {b}`"
                    ));
                }
                Some(old) if files.len() as u64 > *old => {
                    return Err(format!(
                        "--update-baseline refused for crate `{crate_name}`: pair `{a} -> {b}` grew from {old} to {}",
                        files.len()
                    ));
                }
                Some(_) => {}
            }
        }
    }
    Ok(())
}

/// The baseline to write: only measured pairs with count > 0; crates
/// with no measured pairs are omitted.
fn written_baseline(measured: &BTreeMap<String, Pairs>) -> BTreeMap<String, BTreeMap<String, u64>> {
    let mut out = BTreeMap::new();
    for (crate_name, pairs) in measured {
        let mut section = BTreeMap::new();
        for ((a, b), files) in pairs {
            if files.is_empty() {
                continue;
            }
            section.insert(pair_key(a, b), files.len() as u64);
        }
        if !section.is_empty() {
            out.insert(crate_name.clone(), section);
        }
    }
    out
}
