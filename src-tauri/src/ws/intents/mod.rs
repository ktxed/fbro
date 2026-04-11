mod app;
pub(crate) mod bro;
mod jobs;
mod navigation;
mod panel;
mod viewer;

use serde_json::Value;

use crate::ws::runtime::IntentContext;

pub async fn dispatch(name: &str, scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    match name {
        "RefreshPanels"
        | "SetSort"
        | "SetFilter"
        | "SetFilterMode"
        | "MoveCursor"
        | "SetSelection"
        | "SelectAtIndex"
        | "DeselectAtIndex"
        | "ToggleSelectionAtIndex"
        | "ToggleSelection"
        | "SelectAll"
        | "ClearSelection"
        | "InvertSelection"
        | "MoveCursorWithSelect"
        | "MoveCursorBy"
        | "MoveCursorToEdge"
        | "SetCursor"
        | "CalculateSize"
        | "CancelSizeCalculation"
        | "RequestWindow"
        | "SwitchFocus"
        | "RequestStateSnapshot" => panel::handle(name, scope, args, ctx).await,
        "OpenEntry"
        | "NavigateHome"
        | "NavigateSourceRoot"
        | "NavigatePanelTo"
        | "OpenFile"
        | "OpenTerminal"
        | "NavigateToSource"
        | "MountSource"
        | "NavigateUp"
        | "RevealInFileManager"
        | "ListOpenWithApps"
        | "LaunchOpenWithApp" => navigation::handle(name, scope, args, ctx).await,
        "ListBros" | "LaunchBro" => bro::handle(name, scope, args, ctx).await,
        "StartFileOp" | "PasteExternalClipboard" | "WriteExternalClipboard"
        | "ProvideJobDecision" | "CancelJob"
        | "PauseJob" | "ResumeJob" | "RenameFile" | "MakeDir" => {
            jobs::handle(name, scope, args, ctx).await
        }
        "ViewFile" | "EditFile" | "CloseViewerWindow" | "ViewerNavigate" | "ReadFileChunk" => {
            viewer::handle(name, args, ctx).await
        }
        "Quit" | "ConfirmQuit" => app::handle(name, ctx).await,
        unknown => eprintln!("[fbro] Unknown intent: {unknown}"),
    }
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn arg_bool(args: &Value, key: &str, default: bool) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn arg_u64(args: &Value, key: &str, default: u64) -> u64 {
    args.get(key).and_then(Value::as_u64).unwrap_or(default)
}

fn arg_i64(args: &Value, key: &str, default: i64) -> i64 {
    args.get(key).and_then(Value::as_i64).unwrap_or(default)
}

fn arg_usize_vec(args: &Value, key: &str) -> Vec<usize> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_u64().map(|index| index as usize))
                .collect()
        })
        .unwrap_or_default()
}
