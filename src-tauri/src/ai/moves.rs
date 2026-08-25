//! Moving whole files between the two machines, as a step of the loop.
//!
//! Everything else the assistant does goes through the shared terminal; this does
//! not. A transfer is SFTP on its own connection, so size and binary content are
//! no object and nothing has to survive being typed into a shell.
//!
//! The two questions only a window can answer -- which files, and which folder --
//! are asked through [`TransferContext`], so the resolution logic here stays
//! testable against a fake.

use std::future::Future;

use super::types::{CommandResult, TransferKind};

const MAX_FAILURE_LINES: usize = 10;

/// What the model asked for, straight out of the protocol.
#[derive(Debug, Clone)]
pub struct TransferRequest {
    pub kind: TransferKind,
    /// Sources. Empty is allowed for an upload: the user is then asked to pick.
    pub paths: Vec<String>,
    /// The destination directory. Empty means "use the default, or ask".
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferRoot {
    pub path: String,
    pub is_directory: bool,
}

/// A request resolved against both machines and ready to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferPlan {
    pub kind: TransferKind,
    pub roots: Vec<TransferRoot>,
    pub target: String,
}

/// A failure the model can do something about: a path that is not there, a
/// destination that is a file, a dialog the user dismissed. Carried back as an
/// observation rather than raised, because the task is not over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferOpError(pub String);

impl std::fmt::Display for TransferOpError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(&self.0)
    }
}

fn fail<T>(message: impl Into<String>) -> Result<T, TransferOpError> {
    Err(TransferOpError(message.into()))
}

/// What one side of a transfer can be asked about a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    File,
    Directory,
}

/// The two sides, and the two questions only a window can ask.
pub trait TransferContext: Send + Sync {
    /// Whether the path is there on the side being addressed, and what it is.
    fn probe_source(&self, path: &str) -> impl Future<Output = Option<PathKind>> + Send;
    fn probe_target(&self, path: &str) -> impl Future<Output = Option<PathKind>> + Send;
    /// The path as that side spells it: separators, and nothing else.
    fn normalize_source(&self, path: &str) -> String;
    fn normalize_target(&self, path: &str) -> String;
    /// The home directory of the side being addressed, for expanding `~`.
    fn home_source(&self) -> String;
    fn home_target(&self) -> String;
    /// Where an upload goes when the model named nowhere: the terminal's
    /// directory. Asynchronous because the only trustworthy answer comes from
    /// asking the shell.
    fn default_target(&self) -> impl Future<Output = String> + Send;
    /// Opens a file/folder picker. Resolves to `[]` when the user dismissed it.
    fn pick_sources(&self) -> impl Future<Output = Vec<String>> + Send;
    /// Opens a folder picker. Resolves to `""` when the user dismissed it.
    fn pick_target(&self) -> impl Future<Output = String> + Send;
}

/// `~` is the user's home on whichever side is being addressed, and neither side
/// expands it: the remote one because SFTP has no shell to do it, the local one
/// because the transfer page's address bar never sees a `~`.
fn expand_home(target: &str, home: &str, separator: char) -> String {
    if target != "~" && !target.starts_with("~/") && !target.starts_with("~\\") {
        return target.to_string();
    }
    let rest = target[1..].trim_start_matches(['/', '\\']);
    if rest.is_empty() {
        home.to_string()
    } else {
        format!("{}{separator}{rest}", home.trim_end_matches(['/', '\\']))
    }
}

/// Whether each path is there, and whether it is a directory. The model does not
/// get to say.
async fn probe_all<C: TransferContext>(paths: &[String], context: &C) -> Vec<TransferRoot> {
    let mut roots = Vec::new();
    for candidate in paths {
        if let Some(kind) = context.probe_source(candidate).await {
            roots.push(TransferRoot {
                path: candidate.clone(),
                is_directory: kind == PathKind::Directory,
            });
        }
    }
    roots
}

async fn resolve_sources<C: TransferContext>(
    request: &TransferRequest,
    context: &C,
) -> Result<Vec<TransferRoot>, TransferOpError> {
    let home = context.home_source();
    let asked: Vec<String> = request
        .paths
        .iter()
        .map(|candidate| {
            expand_home(
                &context.normalize_source(candidate),
                &home,
                separator_of(&home),
            )
        })
        .filter(|candidate| !candidate.is_empty())
        .collect();

    if asked.is_empty() {
        return picked_sources(request, context).await;
    }

    /*
     * Every named path is stat'd here, now, however confidently it was named. The
     * model's belief about what is on a disk comes from output it read some steps
     * ago, and the user can create and delete files in between -- so what it
     * believes is a claim about the past, and this is the present.
     *
     * A miss fails the whole action rather than quietly moving the rest. Silently
     * transferring two of three files and reporting success is how a missing file
     * goes unnoticed until it is needed.
     */
    let roots = probe_all(&asked, context).await;
    let missing: Vec<&String> = asked
        .iter()
        .filter(|candidate| !roots.iter().any(|root| root.path == **candidate))
        .collect();
    if missing.is_empty() {
        return Ok(roots);
    }

    let where_ = if request.kind == TransferKind::Download {
        "on the server"
    } else {
        "on the user's computer"
    };
    let next = if request.kind == TransferKind::Download {
        "Check with ls and send the action again with the real path."
    } else {
        "You cannot list that machine: ask the user for the exact path, or send the action with no \
         \"path\" at all and they are shown a file picker."
    };
    let names: Vec<&str> = missing.iter().map(|path| path.as_str()).collect();
    fail(format!(
        "Nothing was transferred. These do not exist {where_}: {}. {next}",
        names.join(", ")
    ))
}

/// The upload fallback: the user points at what the model could not name.
async fn picked_sources<C: TransferContext>(
    request: &TransferRequest,
    context: &C,
) -> Result<Vec<TransferRoot>, TransferOpError> {
    if request.kind == TransferKind::Download {
        return fail("The download action needs a \"path\" -- the file or folder on the server.");
    }
    let picked = context.pick_sources().await;
    if picked.is_empty() {
        return fail(
            "The user closed the file picker without choosing anything, so nothing was uploaded. \
             Ask them which file they meant, or carry on with something else.",
        );
    }
    let chosen = probe_all(&picked, context).await;
    if chosen.is_empty() {
        return fail("Nothing readable was chosen, so nothing was uploaded.");
    }
    Ok(chosen)
}

async fn resolve_target<C: TransferContext>(
    request: &TransferRequest,
    context: &C,
) -> Result<String, TransferOpError> {
    let named = request.to.trim();
    if !named.is_empty() {
        let home = context.home_target();
        return Ok(expand_home(
            &context.normalize_target(named),
            &home,
            separator_of(&home),
        ));
    }

    /*
     * An upload has a default worth using: the directory the user is standing in
     * in the terminal, which is where "put it here" means. A download has none --
     * the assistant has never seen this disk -- so the folder picker is the answer
     * rather than a guess at Downloads.
     */
    if request.kind == TransferKind::Upload {
        let fallback = context.default_target().await;
        let fallback = fallback.trim();
        if !fallback.is_empty() {
            return Ok(fallback.to_string());
        }
    }

    let picked = context.pick_target().await;
    if picked.is_empty() {
        return fail(
            "The user closed the folder picker without choosing anywhere, so nothing was \
             transferred. Ask them where it should go, or carry on with something else.",
        );
    }
    Ok(picked)
}

/// Which separator a side spells its paths with, taken from its own home.
fn separator_of(home: &str) -> char {
    if home.contains('\\') && !home.contains('/') {
        '\\'
    } else {
        '/'
    }
}

/// Works out exactly what will move and where it will land.
///
/// Everything here is read-only on both machines: the destination must already
/// exist, and nothing creates it.
pub async fn plan_transfer<C: TransferContext>(
    request: &TransferRequest,
    context: &C,
) -> Result<TransferPlan, TransferOpError> {
    let roots = resolve_sources(request, context).await?;
    let target = resolve_target(request, context).await?;

    /*
     * The destination must already be there. Creating it would turn one mistyped
     * character into a directory nobody asked for, holding files the user will
     * look for in the folder they actually meant and never find -- and unlike a
     * refusal, that failure is silent and is discovered much later.
     *
     * Recovering is cheap on both sides: on the server the model can mkdir once
     * the user has agreed, and locally the folder picker creates folders.
     */
    match context.probe_target(&target).await {
        None => {
            let next = if request.kind == TransferKind::Upload {
                "Create it first if that is really where this belongs, or pick a directory that \
                 exists."
            } else {
                "Send the action with no \"to\" and the user is shown a folder picker, where they \
                 can make a new folder if they want one -- or name a directory that already exists."
            };
            fail(format!(
                "Nothing was transferred. {target} does not exist. {next}"
            ))
        }
        Some(PathKind::File) => fail(format!(
            "{target} is a file, not a directory. \"to\" is the folder the items are put in, not \
             the name to give them."
        )),
        Some(PathKind::Directory) => Ok(TransferPlan {
            kind: request.kind,
            roots,
            target,
        }),
    }
}

/// What a finished transfer amounted to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TransferSummary {
    pub completed: u32,
    pub skipped: u32,
    pub failed: u32,
    pub cancelled: bool,
}

/// What the model is told about a finished transfer.
///
/// Skips are the part worth spelling out. The model did not ask for them and
/// would otherwise read "3 skipped" as a detail rather than as "those three files
/// are not where you think they are", so the sentence says what did not happen,
/// why, and what the next move is.
pub fn describe_transfer(
    plan: &TransferPlan,
    summary: &TransferSummary,
    skipped: &[String],
    failures: &[String],
) -> CommandResult {
    let direction = match plan.kind {
        TransferKind::Upload => "Uploaded",
        TransferKind::Download => "Downloaded",
    };
    let mut lines = vec![format!(
        "{direction} {} file(s) to {}.",
        summary.completed, plan.target
    )];

    if summary.cancelled {
        lines.push(
            "The user stopped the transfer, so the rest was not moved. Do not start it again \
             unless they ask."
                .to_string(),
        );
    }
    if summary.skipped > 0 {
        lines.push(String::new());
        lines.push(format!(
            "{} item(s) were NOT transferred because something with that name is already at the",
            summary.skipped
        ));
        lines.push("destination. Nothing there was overwritten.".to_string());
        lines.extend(
            skipped
                .iter()
                .take(MAX_FAILURE_LINES)
                .map(|path| format!("  {path}")),
        );
        lines.push(
            "If those files are the point of the task, put them somewhere else, or rename or \
             remove what is"
                .to_string(),
        );
        lines.push("already there first -- and say so before you do.".to_string());
    }
    if summary.failed > 0 {
        lines.push(String::new());
        lines.push(format!("{} item(s) failed:", summary.failed));
        lines.extend(
            failures
                .iter()
                .take(MAX_FAILURE_LINES)
                .map(|line| format!("  {line}")),
        );
    }

    // Nothing moved and something went wrong is a failed step; a clean run in
    // which everything was already there is not, and must not be reported as one.
    let failed_outright = summary.failed > 0 && summary.completed == 0;
    CommandResult {
        output: lines.join("\n"),
        exit_code: if failed_outright { 1 } else { 0 },
        timed_out: false,
        truncated: false,
    }
}

/// The local directories worth naming to the model.
///
/// Without these it cannot honour "download it to my desktop" -- it is told not
/// to guess local paths, and a path it has never been given is a guess. So the
/// few places a user actually names out loud are read off this machine and put in
/// front of it: not to let it wander the disk, but so the handful of folders they
/// will ask for by name resolve without a dialog.
///
/// Every entry is checked before it is offered. A Desktop that is not there is
/// worse than no Desktop at all: the transfer would create the folder and put the
/// file somewhere the user does not look.
pub fn describe_local_places(last_used: &str) -> Vec<String> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let mut lines = vec![format!("  home: {}", home.display())];

    for (label, dir) in [
        ("desktop", dirs::desktop_dir()),
        ("downloads", dirs::download_dir()),
    ] {
        // `dirs` reads the platform's own answer, which is what knows that
        // Desktop is called 桌面 on this account and that OneDrive has moved it.
        if let Some(found) = dir.filter(|path| path.is_dir()) {
            lines.push(format!("  {label}: {}", found.display()));
        }
    }

    if !last_used.is_empty() && std::path::Path::new(last_used).is_dir() {
        lines.push(format!(
            "  where they last put a downloaded file: {last_used}"
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[derive(Default)]
    struct Fake {
        source: HashMap<String, PathKind>,
        target: HashMap<String, PathKind>,
        default_target: String,
        picked_sources: Vec<String>,
        picked_target: String,
    }

    impl TransferContext for Fake {
        async fn probe_source(&self, path: &str) -> Option<PathKind> {
            self.source.get(path).copied()
        }
        async fn probe_target(&self, path: &str) -> Option<PathKind> {
            self.target.get(path).copied()
        }
        fn normalize_source(&self, path: &str) -> String {
            path.trim().to_string()
        }
        fn normalize_target(&self, path: &str) -> String {
            path.trim().to_string()
        }
        fn home_source(&self) -> String {
            "/home/me".into()
        }
        fn home_target(&self) -> String {
            "/home/me".into()
        }
        async fn default_target(&self) -> String {
            self.default_target.clone()
        }
        async fn pick_sources(&self) -> Vec<String> {
            self.picked_sources.clone()
        }
        async fn pick_target(&self) -> String {
            self.picked_target.clone()
        }
    }

    fn download(paths: &[&str], to: &str) -> TransferRequest {
        TransferRequest {
            kind: TransferKind::Download,
            paths: paths.iter().map(|path| path.to_string()).collect(),
            to: to.into(),
        }
    }

    fn upload(paths: &[&str], to: &str) -> TransferRequest {
        TransferRequest {
            kind: TransferKind::Upload,
            paths: paths.iter().map(|path| path.to_string()).collect(),
            to: to.into(),
        }
    }

    #[test]
    fn a_tilde_becomes_the_home_of_the_side_being_addressed() {
        assert_eq!(expand_home("~", "/home/me", '/'), "/home/me");
        assert_eq!(expand_home("~/logs", "/home/me", '/'), "/home/me/logs");
        assert_eq!(
            expand_home("~\\logs", "C:\\Users\\me", '\\'),
            "C:\\Users\\me\\logs"
        );
        assert_eq!(expand_home("/absolute", "/home/me", '/'), "/absolute");
        assert_eq!(expand_home("~notuser", "/home/me", '/'), "~notuser");
    }

    #[tokio::test]
    async fn every_named_source_is_checked_before_anything_moves() {
        let mut fake = Fake::default();
        fake.source.insert("/var/log/a".into(), PathKind::File);
        fake.target.insert("/tmp".into(), PathKind::Directory);

        let plan = plan_transfer(&download(&["/var/log/a"], "/tmp"), &fake)
            .await
            .unwrap();
        assert_eq!(
            plan.roots,
            vec![TransferRoot {
                path: "/var/log/a".into(),
                is_directory: false
            }]
        );
        assert_eq!(plan.target, "/tmp");
    }

    #[tokio::test]
    async fn one_missing_source_fails_the_whole_action() {
        let mut fake = Fake::default();
        fake.source.insert("/var/log/a".into(), PathKind::File);
        fake.target.insert("/tmp".into(), PathKind::Directory);

        let error = plan_transfer(&download(&["/var/log/a", "/var/log/gone"], "/tmp"), &fake)
            .await
            .unwrap_err();
        assert!(error.0.contains("Nothing was transferred"));
        assert!(error.0.contains("/var/log/gone"));
        assert!(
            !error.0.contains("/var/log/a,"),
            "the one that exists is not listed as missing"
        );
    }

    #[tokio::test]
    async fn a_destination_that_is_not_there_is_refused_rather_than_created() {
        let mut fake = Fake::default();
        fake.source.insert("/a".into(), PathKind::File);
        let error = plan_transfer(&download(&["/a"], "/nowhere"), &fake)
            .await
            .unwrap_err();
        assert!(error.0.contains("/nowhere does not exist"));
        assert!(error.0.contains("folder picker"));
    }

    #[tokio::test]
    async fn a_destination_that_is_a_file_says_what_to_is_for() {
        let mut fake = Fake::default();
        fake.source.insert("/a".into(), PathKind::File);
        fake.target.insert("/tmp/x".into(), PathKind::File);
        let error = plan_transfer(&download(&["/a"], "/tmp/x"), &fake)
            .await
            .unwrap_err();
        assert!(error.0.contains("is a file, not a directory"));
        assert!(error.0.contains("not the name to give them"));
    }

    #[tokio::test]
    async fn an_upload_with_no_destination_lands_where_the_shell_is_standing() {
        let mut fake = Fake::default();
        fake.source.insert("/local/a".into(), PathKind::File);
        fake.target.insert("/srv/app".into(), PathKind::Directory);
        fake.default_target = "/srv/app".into();

        let plan = plan_transfer(&upload(&["/local/a"], ""), &fake)
            .await
            .unwrap();
        assert_eq!(plan.target, "/srv/app");
    }

    #[tokio::test]
    async fn a_download_with_no_destination_asks_rather_than_guessing() {
        let mut fake = Fake::default();
        fake.source.insert("/a".into(), PathKind::File);
        fake.target.insert("/picked".into(), PathKind::Directory);
        fake.picked_target = "/picked".into();

        let plan = plan_transfer(&download(&["/a"], ""), &fake).await.unwrap();
        assert_eq!(plan.target, "/picked");
    }

    #[tokio::test]
    async fn a_dismissed_folder_picker_transfers_nothing() {
        let mut fake = Fake::default();
        fake.source.insert("/a".into(), PathKind::File);
        let error = plan_transfer(&download(&["/a"], ""), &fake)
            .await
            .unwrap_err();
        assert!(error.0.contains("closed the folder picker"));
    }

    #[tokio::test]
    async fn an_upload_with_no_path_shows_a_file_picker() {
        let mut fake = Fake::default();
        fake.picked_sources = vec!["/local/chosen".into()];
        fake.source.insert("/local/chosen".into(), PathKind::File);
        fake.target.insert("/srv".into(), PathKind::Directory);

        let plan = plan_transfer(&upload(&[], "/srv"), &fake).await.unwrap();
        assert_eq!(plan.roots[0].path, "/local/chosen");
    }

    #[tokio::test]
    async fn a_download_with_no_path_has_nothing_to_pick_from() {
        let fake = Fake::default();
        let error = plan_transfer(&download(&[], "/tmp"), &fake)
            .await
            .unwrap_err();
        assert!(error.0.contains("needs a \"path\""));
    }

    #[tokio::test]
    async fn a_dismissed_file_picker_uploads_nothing() {
        let fake = Fake::default();
        let error = plan_transfer(&upload(&[], "/srv"), &fake)
            .await
            .unwrap_err();
        assert!(error.0.contains("closed the file picker"));
    }

    fn plan() -> TransferPlan {
        TransferPlan {
            kind: TransferKind::Download,
            roots: vec![TransferRoot {
                path: "/a".into(),
                is_directory: false,
            }],
            target: "/tmp".into(),
        }
    }

    #[test]
    fn a_clean_transfer_reports_what_moved() {
        let summary = TransferSummary {
            completed: 3,
            ..Default::default()
        };
        let result = describe_transfer(&plan(), &summary, &[], &[]);
        assert_eq!(result.output, "Downloaded 3 file(s) to /tmp.");
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn skips_are_spelled_out_with_what_to_do_about_them() {
        let summary = TransferSummary {
            completed: 1,
            skipped: 2,
            ..Default::default()
        };
        let result = describe_transfer(&plan(), &summary, &["/tmp/a".into(), "/tmp/b".into()], &[]);
        assert!(result.output.contains("2 item(s) were NOT transferred"));
        assert!(result.output.contains("Nothing there was overwritten."));
        assert!(result.output.contains("  /tmp/a"));
        assert_eq!(result.exit_code, 0, "a skip is not a failure");
    }

    #[test]
    fn nothing_moved_and_something_failed_is_a_failed_step() {
        let summary = TransferSummary {
            completed: 0,
            failed: 2,
            ..Default::default()
        };
        let result = describe_transfer(&plan(), &summary, &[], &["FAILED /a".into()]);
        assert_eq!(result.exit_code, 1);
        assert!(result.output.contains("2 item(s) failed:"));

        let partial = TransferSummary {
            completed: 1,
            failed: 1,
            ..Default::default()
        };
        assert_eq!(describe_transfer(&plan(), &partial, &[], &[]).exit_code, 0);
    }

    #[test]
    fn a_cancelled_transfer_says_not_to_start_it_again() {
        let summary = TransferSummary {
            completed: 1,
            cancelled: true,
            ..Default::default()
        };
        let result = describe_transfer(&plan(), &summary, &[], &[]);
        assert!(result.output.contains("The user stopped the transfer"));
    }

    #[test]
    fn the_local_places_always_name_home_at_least() {
        let lines = describe_local_places("");
        assert!(lines.iter().any(|line| line.starts_with("  home: ")));
        assert!(!lines
            .iter()
            .any(|line| line.contains("where they last put")));
    }
}
