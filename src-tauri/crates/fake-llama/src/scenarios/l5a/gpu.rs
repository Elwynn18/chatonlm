//! Autocomplétion de l'éditeur (`/infill`) et visite guidée (lot L5a).

use serde_json::{Value, json};

use super::super::{ChatCall, Custom, InfillCall, Script, Step};

/// Proposition sur plusieurs lignes qui dit ce qu'elle a reçu :
/// `value(<préfixe>|<suffixe>)` (longueurs en caractères), une deuxième
/// ligne, une ligne vide, puis une suite jamais montrée. Comme le vrai :
/// coupée au premier arrêt (`stop`, fin `word`), puis à `n_predict`
/// morceaux de 4 caractères (fin `limit`).
pub const FIM_LINES: Custom = Custom {
    name: "chatonlm-gpu-fimlines",
    chat: answer,
    infill: Some(lines),
    load_ms: None,
    fail_load: false,
};

/// Proposition qui n'arrive qu'au bout de 5 s (abandon de l'app).
pub const FIM_WAIT: Custom = Custom {
    name: "chatonlm-gpu-fimwait",
    chat: answer,
    infill: Some(wait),
    load_ms: None,
    fail_load: false,
};

/// Comme [`FIM_LINES`], chargé en 1,5 s.
pub const FIM_SLOW_LOAD: Custom = Custom {
    name: "chatonlm-gpu-fimslowload",
    chat: answer,
    infill: Some(lines),
    load_ms: Some(1500),
    fail_load: false,
};

/// Visite guidée : un plan JSON dans les lignes que borne le schéma ; un
/// plan sans segment (inutilisable) si le message de l'utilisateur contient
/// `UNUSABLE`.
pub const TOUR: Custom = Custom {
    name: "chatonlm-gpu-tour",
    chat: tour,
    infill: None,
    load_ms: None,
    fail_load: false,
};

/// Comme [`TOUR`], pour un modèle qui réfléchit.
pub const TOUR_THINK: Custom = Custom {
    name: "chatonlm-gpu-tourthink",
    chat: tour,
    infill: None,
    load_ms: None,
    fail_load: false,
};

fn answer(_: &ChatCall) -> Script {
    vec![Step::Content("ok".into())]
}

fn text(body: &Value, key: &str) -> usize {
    body[key].as_str().unwrap_or_default().chars().count()
}

fn lines(call: &InfillCall) -> Script {
    let full = format!(
        "value({}|{})\n    second line\n\nnever shown",
        text(call.body, "input_prefix"),
        text(call.body, "input_suffix")
    );
    let mut content = full.clone();
    let mut finish = "eos";
    let stops = call.body["stop"].as_array().cloned().unwrap_or_default();
    if let Some(cut) = stops
        .iter()
        .filter_map(Value::as_str)
        .filter(|stop| !stop.is_empty())
        .filter_map(|stop| full.find(stop))
        .min()
    {
        content.truncate(cut);
        finish = "word";
    }
    let pieces: Vec<String> = content
        .chars()
        .collect::<Vec<_>>()
        .chunks(4)
        .map(|chunk| chunk.iter().collect())
        .collect();
    let limit = call.body["n_predict"]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok());
    let mut script: Script = match limit {
        Some(limit) if pieces.len() > limit => {
            finish = "limit";
            pieces[..limit].iter().cloned().map(Step::Content).collect()
        }
        _ => pieces.into_iter().map(Step::Content).collect(),
    };
    script.push(Step::Finish(finish));
    script
}

fn wait(_: &InfillCall) -> Script {
    vec![
        Step::Wait(std::time::Duration::from_secs(5)),
        Step::Content("late".into()),
    ]
}

fn tour(call: &ChatCall) -> Script {
    let messages = call.body["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let user = messages
        .iter()
        .rev()
        .find(|message| message["role"] == "user")
        .and_then(|message| message["content"].as_str())
        .unwrap_or_default()
        .to_owned();
    let schema = &call.body["response_format"]["json_schema"]["schema"];
    let lines = &schema["properties"]["segments"]["items"]["properties"]["lines"]["items"];
    let first = lines["minimum"].as_u64().unwrap_or(1);
    let last = lines["maximum"].as_u64().unwrap_or(first);
    let plan = if user.contains("UNUSABLE") {
        json!({ "segments": [] })
    } else {
        json!({ "segments": [
            { "lines": [first, first], "text": "This line starts it." },
            { "lines": [first, last], "text": "And this is the whole." },
        ] })
    };
    vec![Step::Content(plan.to_string())]
}
