use std::collections::HashMap;

use clap::{Arg, ArgAction, ArgMatches, Command};

use crate::api::schema::workspaces::{
    WorkspaceCloseProjection, WorkspaceCreateProjection, WorkspaceMetadataProjection,
    WorkspaceRenameProjection,
};
use crate::api::schema::{
    EmptyParams, Method, Request, WorkspaceCreateChildParams, WorkspaceInfo,
    WorkspaceSetParentParams, WorkspaceTarget,
};
use crate::workspace_tree::{plan_workspace_move, WorkspaceMovePlacement, WorkspaceNode};

use super::projection::{cli_projection, Field, Input, ParseError};

pub(crate) use super::projection::{boolean, optional_text};

pub(crate) fn workspace_id() -> Input<String> {
    Input::positional("workspace_id", 0, |slot, value| {
        *slot = super::normalize_workspace_id(value);
        Ok(())
    })
}

pub(crate) fn label() -> Input<String> {
    Input::rest("label", "LABEL", 1, |slot, value| {
        *slot = value.to_owned();
        Ok(())
    })
    .verbatim()
}

pub(crate) fn environment() -> Input<HashMap<String, String>> {
    Input::<HashMap<String, String>>::value("env", "KEY=VALUE", |slot, value| {
        let (key, value) = super::parse_env_assignment(value)?;
        slot.insert(key, value);
        Ok(())
    })
    .help("Set an environment variable for the launched process")
}

pub(crate) fn source() -> Input<String> {
    Input::value("source", "ID", |slot, value| {
        *slot = value.to_owned();
        Ok(())
    })
    .required()
}

pub(crate) fn validate_source(source: &str) -> Result<(), ParseError> {
    if source.trim().is_empty() {
        return Err(ParseError::Usage("missing required --source".into()));
    }
    Ok(())
}

pub(crate) fn token() -> Input<HashMap<String, Option<String>>> {
    Input::<HashMap<String, Option<String>>>::value("token", "NAME=VALUE", |slot, value| {
        let (key, value) = super::parse_token_assignment(value)?;
        slot.insert(key, value);
        Ok(())
    })
}

pub(crate) fn clear_token() -> Input<HashMap<String, Option<String>>> {
    Input::<HashMap<String, Option<String>>>::value("clear-token", "NAME", |slot, value| {
        slot.insert(value.to_owned(), None);
        Ok(())
    })
}

pub(crate) fn validate_tokens(tokens: &HashMap<String, Option<String>>) -> Result<(), ParseError> {
    if tokens.is_empty() {
        return Err(ParseError::Usage("missing token to set or clear".into()));
    }
    Ok(())
}

pub(crate) fn seq() -> Input<Option<u64>> {
    Input::value("seq", "N", |slot, value| {
        *slot = Some(super::parse_u64_flag("--seq", value)?);
        Ok(())
    })
}

pub(crate) fn ttl_ms() -> Input<Option<u64>> {
    Input::value("ttl-ms", "N", |slot, value| {
        *slot = Some(super::parse_u64_flag("--ttl-ms", value)?);
        Ok(())
    })
}

cli_projection! {
    pub(crate) WorkspaceTargetProjection for WorkspaceTarget {
        workspace_id => Field::new(String::new()).input(workspace_id()),
    }
}

cli_projection! {
    pub(crate) WorkspaceListProjection for EmptyParams {}
}

pub(crate) struct Leaf {
    pub(crate) name: &'static str,
    about: &'static str,
    id: &'static str,
    pub(crate) quiet: bool,
    arguments: fn() -> Vec<Arg>,
    method: fn(&ArgMatches) -> Result<Method, ParseError>,
}

impl Leaf {
    pub(crate) fn request(&self, args: &[String]) -> Result<Request, ParseError> {
        let method = super::projection::parse(
            self.command()
                .bin_name(format!("herdr workspace {}", self.name)),
            args,
            self.method,
        )?;
        Ok(Request {
            id: self.id.into(),
            method,
        })
    }

    fn command(&self) -> Command {
        super::projection::command(self.name, (self.arguments)()).about(self.about)
    }
}

fn create_arguments() -> Vec<Arg> {
    let mut arguments = WorkspaceCreateProjection::arguments();
    arguments.push(
        Arg::new("parent")
            .long("parent")
            .value_name("WORKSPACE_ID")
            .help("Create the workspace as a child of this workspace"),
    );
    arguments
}

fn create_method(matches: &ArgMatches) -> Result<Method, ParseError> {
    let create = WorkspaceCreateProjection::from_matches(matches)?;
    Ok(match matches.get_one::<String>("parent") {
        Some(parent) => Method::WorkspaceCreateChild(WorkspaceCreateChildParams {
            parent_workspace_id: super::normalize_workspace_id(parent),
            create,
        }),
        None => Method::WorkspaceCreate(create),
    })
}

fn set_parent_arguments() -> Vec<Arg> {
    vec![
        Arg::new("workspace_id")
            .value_name("WORKSPACE_ID")
            .required(true),
        Arg::new("parent")
            .long("parent")
            .value_name("WORKSPACE_ID")
            .help("Workspace to become the parent")
            .required_unless_present("clear")
            .conflicts_with("clear"),
        Arg::new("clear")
            .long("clear")
            .action(ArgAction::SetTrue)
            .help("Remove the parent link"),
    ]
}

fn set_parent_method(matches: &ArgMatches) -> Result<Method, ParseError> {
    let workspace_id = required_id(matches, "workspace_id")?;
    Ok(Method::WorkspaceSetParent(WorkspaceSetParentParams {
        workspace_id,
        parent_workspace_id: matches
            .get_one::<String>("parent")
            .map(|parent| super::normalize_workspace_id(parent)),
    }))
}

fn required_id(matches: &ArgMatches, name: &str) -> Result<String, ParseError> {
    matches
        .get_one::<String>(name)
        .map(|value| super::normalize_workspace_id(value))
        .ok_or_else(|| ParseError::Usage(format!("missing required {name}")))
}

fn move_arguments() -> Vec<Arg> {
    vec![
        Arg::new("workspace_id")
            .value_name("WORKSPACE_ID")
            .required(true),
        Arg::new("before")
            .long("before")
            .value_name("WORKSPACE_ID")
            .help("Place before this workspace"),
        Arg::new("after")
            .long("after")
            .value_name("WORKSPACE_ID")
            .help("Place after this workspace"),
        Arg::new("index")
            .long("index")
            .value_name("N")
            .value_parser(clap::value_parser!(usize))
            .help("Zero-based position among workspaces of the same level"),
    ]
}

fn move_placement(matches: &ArgMatches) -> Result<WorkspaceMovePlacement, ParseError> {
    let before = matches.get_one::<String>("before");
    let after = matches.get_one::<String>("after");
    let index = matches.get_one::<usize>("index");
    match (before, after, index) {
        (Some(target), None, None) => Ok(WorkspaceMovePlacement::Before(
            super::normalize_workspace_id(target),
        )),
        (None, Some(target), None) => Ok(WorkspaceMovePlacement::After(
            super::normalize_workspace_id(target),
        )),
        (None, None, Some(index)) => Ok(WorkspaceMovePlacement::Index(*index)),
        _ => Err(ParseError::Usage(
            "exactly one of --before, --after or --index is required".into(),
        )),
    }
}

/// The move depends on the current tree, so the plan starts from a fresh list.
fn move_method(matches: &ArgMatches) -> Result<Method, ParseError> {
    let source = required_id(matches, "workspace_id")?;
    let placement = move_placement(matches)?;
    let response = super::send_request(&Request {
        id: "cli:workspace:move".into(),
        method: Method::WorkspaceList(EmptyParams::default()),
    })?;
    if let Some(error) = response.get("error") {
        return Err(ParseError::Usage(error.to_string()));
    }
    let workspaces: Vec<WorkspaceInfo> = response
        .pointer("/result/workspaces")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| ParseError::Usage(format!("invalid workspace list: {error}")))?
        .unwrap_or_default();
    plan_move(&workspaces, &source, &placement)
}

/// Accepts the id forms the server accepts: the id itself, `w_N` and `N`.
fn resolve_workspace_id<'a>(workspaces: &'a [WorkspaceInfo], requested: &'a str) -> &'a str {
    workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == requested)
        .or_else(|| {
            let number = requested
                .strip_prefix("w_")
                .unwrap_or(requested)
                .parse::<usize>()
                .ok()?;
            workspaces.get(number.checked_sub(1)?)
        })
        .map_or(requested, |workspace| &workspace.workspace_id)
}

fn plan_move(
    workspaces: &[WorkspaceInfo],
    source: &str,
    placement: &WorkspaceMovePlacement,
) -> Result<Method, ParseError> {
    let source = resolve_workspace_id(workspaces, source);
    let placement = &match placement {
        WorkspaceMovePlacement::Before(target) => {
            WorkspaceMovePlacement::Before(resolve_workspace_id(workspaces, target).to_owned())
        }
        WorkspaceMovePlacement::After(target) => {
            WorkspaceMovePlacement::After(resolve_workspace_id(workspaces, target).to_owned())
        }
        other => other.clone(),
    };
    let nodes = workspaces
        .iter()
        .map(|workspace| WorkspaceNode {
            id: &workspace.workspace_id,
            worktree_key: workspace
                .worktree
                .as_ref()
                .map(|worktree| worktree.repo_key.as_str()),
            is_linked_worktree: workspace
                .worktree
                .as_ref()
                .is_some_and(|worktree| worktree.is_linked_worktree),
            parent_id: workspace.parent_workspace_id.as_deref(),
        })
        .collect::<Vec<_>>();
    plan_workspace_move(&nodes, source, placement)
        .map_err(|error| ParseError::Usage(error.0))
        // Already in place: answer like a move that changed nothing, with the current order.
        .map(|method| method.unwrap_or(Method::WorkspaceList(EmptyParams::default())))
}

pub(crate) const LEAVES: [Leaf; 9] = [
    Leaf {
        name: "list",
        about: "List workspaces",
        id: "cli:workspace:list",
        quiet: false,
        arguments: WorkspaceListProjection::arguments,
        method: |matches| WorkspaceListProjection::from_matches(matches).map(Method::WorkspaceList),
    },
    Leaf {
        name: "create",
        about: "Create a workspace",
        id: "cli:workspace:create",
        quiet: false,
        arguments: create_arguments,
        method: create_method,
    },
    Leaf {
        name: "get",
        about: "Show a workspace",
        id: "cli:workspace:get",
        quiet: false,
        arguments: WorkspaceTargetProjection::arguments,
        method: |matches| {
            WorkspaceTargetProjection::from_matches(matches).map(Method::WorkspaceGet)
        },
    },
    Leaf {
        name: "focus",
        about: "Focus a workspace",
        id: "cli:workspace:focus",
        quiet: false,
        arguments: WorkspaceTargetProjection::arguments,
        method: |matches| {
            WorkspaceTargetProjection::from_matches(matches).map(Method::WorkspaceFocus)
        },
    },
    Leaf {
        name: "rename",
        about: "Rename a workspace",
        id: "cli:workspace:rename",
        quiet: false,
        arguments: WorkspaceRenameProjection::arguments,
        method: |matches| {
            WorkspaceRenameProjection::from_matches(matches).map(Method::WorkspaceRename)
        },
    },
    Leaf {
        name: "set-parent",
        about: "Set or clear the parent of a workspace",
        id: "cli:workspace:set-parent",
        quiet: false,
        arguments: set_parent_arguments,
        method: set_parent_method,
    },
    Leaf {
        name: "move",
        about: "Move a workspace with its group among workspaces of the same level",
        id: "cli:workspace:move",
        quiet: false,
        arguments: move_arguments,
        method: move_method,
    },
    Leaf {
        name: "report-metadata",
        about: "Report display-only workspace metadata",
        id: "cli:request",
        quiet: true,
        arguments: WorkspaceMetadataProjection::arguments,
        method: |matches| {
            WorkspaceMetadataProjection::from_matches(matches).map(Method::WorkspaceReportMetadata)
        },
    },
    Leaf {
        name: "close",
        about: "Close a workspace",
        id: "cli:workspace:close",
        quiet: false,
        arguments: WorkspaceCloseProjection::arguments,
        method: |matches| {
            WorkspaceCloseProjection::from_matches(matches).map(Method::WorkspaceClose)
        },
    },
];

pub(crate) fn command() -> Command {
    Command::new("workspace")
        .about("Manage workspaces over the socket API")
        .subcommands(LEAVES.iter().map(Leaf::command))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(command: &str, args: &[&str]) -> Result<Request, ParseError> {
        LEAVES
            .iter()
            .find(|leaf| leaf.name == command)
            .unwrap()
            .request(&args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>())
    }

    #[test]
    fn create_with_parent_uses_the_child_method_and_keeps_create_unchanged() {
        assert_eq!(
            serde_json::to_value(
                request(
                    "create",
                    &["--parent", "w1", "--cwd", "/tmp", "--label", "x"]
                )
                .unwrap()
            )
            .unwrap(),
            serde_json::json!({
                "id": "cli:workspace:create", "method": "workspace.create_child",
                "params": {
                    "parent_workspace_id": "w1", "cwd": "/tmp", "label": "x", "focus": false
                }
            })
        );
    }

    #[test]
    fn set_parent_requires_exactly_one_of_parent_and_clear() {
        assert_eq!(
            serde_json::to_value(request("set-parent", &["w2", "--parent", "w1"]).unwrap())
                .unwrap()["params"],
            serde_json::json!({"workspace_id": "w2", "parent_workspace_id": "w1"})
        );
        assert_eq!(
            serde_json::to_value(request("set-parent", &["w2", "--clear"]).unwrap()).unwrap()
                ["params"],
            serde_json::json!({"workspace_id": "w2", "parent_workspace_id": null})
        );
        assert!(request("set-parent", &["w2"]).is_err());
        assert!(request("set-parent", &["w2", "--parent", "w1", "--clear"]).is_err());
    }

    #[test]
    fn move_accepts_the_id_forms_the_server_accepts() {
        let info = |id: &str, parent: Option<&str>| WorkspaceInfo {
            workspace_id: id.into(),
            number: 0,
            label: id.into(),
            focused: false,
            pane_count: 1,
            tab_count: 1,
            active_tab_id: format!("{id}:t1"),
            agent_status: crate::api::schema::AgentStatus::Unknown,
            tokens: HashMap::new(),
            worktree: None,
            parent_workspace_id: parent.map(str::to_owned),
        };
        let workspaces = [info("w7", None), info("w9", Some("w7")), info("w3", None)];
        for (source, target) in [("w3", "w7"), ("3", "1"), ("w_3", "w_1")] {
            let method = plan_move(
                &workspaces,
                source,
                &WorkspaceMovePlacement::Before(target.into()),
            )
            .unwrap();
            assert!(
                matches!(
                    &method,
                    Method::WorkspaceMove(params)
                        if params.workspace_id == "w3" && params.insert_index == 0
                ),
                "{source} before {target}: {method:?}"
            );
        }
    }

    #[test]
    fn move_placement_requires_exactly_one_target() {
        let matches = |args: &[&str]| {
            super::super::projection::parse(
                super::super::projection::command("move", move_arguments()),
                &args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>(),
                move_placement,
            )
        };
        assert_eq!(
            matches(&["w1", "--before", "w2"]).unwrap(),
            WorkspaceMovePlacement::Before("w2".into())
        );
        assert_eq!(
            matches(&["w1", "--after", "w2"]).unwrap(),
            WorkspaceMovePlacement::After("w2".into())
        );
        assert_eq!(
            matches(&["w1", "--index", "3"]).unwrap(),
            WorkspaceMovePlacement::Index(3)
        );
        assert!(matches(&["w1"]).is_err());
        assert!(matches(&["w1", "--before", "w2", "--index", "1"]).is_err());
    }

    #[test]
    fn workspace_projection_preserves_defaults_and_ordered_maps() {
        let request = request(
            "create",
            &[
                "--cwd",
                "first",
                "--cwd",
                "--literal",
                "--focus",
                "--no-focus",
                "--env",
                "K=first",
                "--env",
                "K=",
                "--env",
                "V=a=b",
            ],
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({
                "id": "cli:workspace:create", "method": "workspace.create",
                "params": { "cwd": "--literal", "focus": false, "env": {"K": "", "V": "a=b"} }
            })
        );
        assert_eq!(
            serde_json::to_value(super::tests::request("create", &[]).unwrap()).unwrap(),
            serde_json::json!({
                "id": "cli:workspace:create", "method": "workspace.create", "params": {"focus": false}
            })
        );
    }

    #[test]
    fn workspace_projection_preserves_joined_labels_and_hidden_group() {
        assert_eq!(
            serde_json::to_value(request("rename", &["--id", "a", "--flag", "b"]).unwrap())
                .unwrap(),
            serde_json::json!({
                "id": "cli:workspace:rename", "method": "workspace.rename",
                "params": {"workspace_id": "--id", "label": "a --flag b"}
            })
        );
        assert_eq!(
            serde_json::to_value(request("close", &["id", "--group"]).unwrap()).unwrap(),
            serde_json::json!({
                "id": "cli:workspace:close", "method": "workspace.close",
                "params": {"workspace_id": "id", "close_group": true}
            })
        );
        let close = command().find_subcommand("close").unwrap().clone();
        assert!(close
            .get_arguments()
            .any(|arg| arg.get_long() == Some("group") && arg.is_hide_set()));
        command().debug_assert();
    }

    #[test]
    fn workspace_projection_preserves_metadata_patches_and_error_channels() {
        assert_eq!(
            serde_json::to_value(
                request(
                    "report-metadata",
                    &[
                        "id",
                        "--source",
                        "  user:jj  ",
                        "--token",
                        "K=a=b",
                        "--clear-token",
                        "K",
                        "--token",
                        "E=",
                        "--seq",
                        "0",
                        "--ttl-ms",
                        "0",
                    ]
                )
                .unwrap()
            )
            .unwrap(),
            serde_json::json!({
                "id": "cli:request", "method": "workspace.report_metadata",
                "params": {"workspace_id": "id", "source": "  user:jj  ", "tokens": {"K": null, "E": ""}, "seq": 0, "ttl_ms": 0}
            })
        );
        assert!(
            matches!(request("report-metadata", &["id", "--source", "valid", "--seq", "bad"]), Err(ParseError::Io(error)) if error.to_string() == "invalid value for --seq: bad")
        );
        assert!(
            matches!(request("report-metadata", &["id", "--source", " "]), Err(ParseError::Usage(message)) if message == "missing required --source")
        );
        assert!(
            matches!(request("report-metadata", &["id", "--source", "valid"]), Err(ParseError::Usage(message)) if message == "missing token to set or clear")
        );
    }

    #[test]
    fn workspace_projection_uses_authoritative_clap_grammar() {
        assert_eq!(
            serde_json::to_value(request("create", &["--cwd=x"]).unwrap()).unwrap(),
            serde_json::json!({
                "id": "cli:workspace:create", "method": "workspace.create", "params": {"cwd": "x", "focus": false}
            })
        );
        assert_eq!(
            serde_json::to_value(request("close", &["--group", "id"]).unwrap()).unwrap(),
            serde_json::json!({
                "id": "cli:workspace:close", "method": "workspace.close", "params": {"workspace_id": "id", "close_group": true}
            })
        );
        assert!(
            matches!(request("get", &[]), Err(ParseError::Clap(error)) if error.kind() == clap::error::ErrorKind::MissingRequiredArgument)
        );
        assert!(
            matches!(request("get", &["id", "extra"]), Err(ParseError::Clap(error)) if error.kind() == clap::error::ErrorKind::UnknownArgument)
        );
        assert!(
            matches!(request("create", &["--cwd"]), Err(ParseError::Clap(error)) if error.exit_code() == 2)
        );
        assert!(
            matches!(request("get", &["id", "--help"]), Err(ParseError::Clap(error)) if error.kind() == clap::error::ErrorKind::DisplayHelp && error.exit_code() == 0)
        );
        assert!(
            matches!(request("create", &["--env", "=value"]), Err(ParseError::Usage(message)) if message == "env key must not be empty")
        );
    }
}
