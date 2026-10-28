use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use serde::{Deserialize, Serialize};
use tracing::info;

/// Errors that can occur when reading or parsing tournament roster mappings.
#[derive(Debug)]
pub enum RosterError {
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl fmt::Display for RosterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error reading roster: {e}"),
            Self::Json(e) => write!(f, "JSON error parsing roster: {e}"),
        }
    }
}

impl std::error::Error for RosterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Json(e) => Some(e),
        }
    }
}

impl From<std::io::Error> for RosterError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for RosterError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

/// Overridden player display information from a tournament roster dump.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RosterEntry {
    /// Clean, un-truncated real player username.
    pub name: String,
    /// Team / clan tag with full special characters (e.g. "-TWA-" or "H-SR").
    pub tag: Option<String>,
}

/// Loads a tournament roster mapping file from the given path.
///
/// Supports multiple input formats:
/// 1. Array of replays/teams with a `players` mapping:
///    ```json
///    [
///      { "tag": "-TWA-", "players": { "1234567": "Player1", "1234568": "Player2" } },
///      { "tag": "RAIN", "players": { "2345678": "Player3" } }
///    ]
///    ```
/// 2. Full Discord webhook payload object containing a `"replays"`, `"teams"`, or `"clans"` array:
///    ```json
///    {
///      "replays": [
///        { "tag": "-TWA-", "players": { "1234567": "Player1" } }
///      ]
///    }
///    ```
/// 3. Flat dictionary mapping account ID strings or ints to player info:
///    ```json
///    {
///      "1234567": { "name": "Player1", "tag": "-TWA-" },
///      "2345678": "Player2"
///    }
///    ```
/// 4. Array of individual player items:
///    ```json
///    [
///      { "id": 1234567, "name": "Player1", "tag": "-TWA-" }
///    ]
///    ```
pub fn load_roster_map<P: AsRef<Path>>(path: P) -> Result<HashMap<i64, RosterEntry>, RosterError> {
    let content = std::fs::read_to_string(path)?;
    parse_roster_json(&content)
}

/// Parses tournament roster mapping from a JSON string.
pub fn parse_roster_json(json_str: &str) -> Result<HashMap<i64, RosterEntry>, RosterError> {
    let val: serde_json::Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(e) => {
            let sanitized = sanitize_trailing_commas(json_str);
            serde_json::from_str(&sanitized).map_err(|_| e)?
        }
    };
    let mut map = HashMap::new();
    parse_roster_value(&val, &mut map);
    info!("Loaded {} tournament roster entries", map.len());
    Ok(map)
}

/// Strips illegal trailing commas before closing braces/brackets in JSON.
pub fn sanitize_trailing_commas(json: &str) -> String {
    let mut result = String::with_capacity(json.len());
    let mut chars = json.chars();
    let mut in_string = false;
    let mut escape = false;

    while let Some(c) = chars.next() {
        if in_string {
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_string = false;
            }
            result.push(c);
        } else {
            match c {
                '"' => {
                    in_string = true;
                    escape = false;
                    result.push(c);
                }
                ',' => {
                    let mut lookahead = chars.clone();
                    let mut is_trailing = false;
                    for nc in lookahead.by_ref() {
                        if nc.is_whitespace() {
                            continue;
                        }
                        if nc == '}' || nc == ']' {
                            is_trailing = true;
                        }
                        break;
                    }
                    if !is_trailing {
                        result.push(c);
                    }
                }
                _ => {
                    result.push(c);
                }
            }
        }
    }
    result
}

fn parse_roster_value(val: &serde_json::Value, map: &mut HashMap<i64, RosterEntry>) {
    match val {
        serde_json::Value::Array(arr) => {
            for item in arr {
                if let Some(obj) = item.as_object() {
                    // Extract tag / clan if specified on this team or player object
                    let tag = obj
                        .get("tag")
                        .or_else(|| obj.get("clan"))
                        .and_then(|v| v.as_str())
                        .map(clean_tag);

                    if let Some(players) = obj.get("players").and_then(|p| p.as_object()) {
                        // Format 1: Replay or team object with "tag" and "players": { "<account_id>": "Name" }
                        for (id_str, p_val) in players {
                            if let Ok(id) = id_str.parse::<i64>() {
                                let entry = match p_val {
                                    serde_json::Value::String(name) => RosterEntry {
                                        name: name.trim().to_string(),
                                        tag: tag.clone(),
                                    },
                                    serde_json::Value::Object(p_obj) => {
                                        let p_name = p_obj
                                            .get("name")
                                            .and_then(|n| n.as_str())
                                            .unwrap_or("")
                                            .trim()
                                            .to_string();
                                        let p_tag = p_obj
                                            .get("tag")
                                            .or_else(|| p_obj.get("clan"))
                                            .and_then(|t| t.as_str())
                                            .map(clean_tag)
                                            .or_else(|| tag.clone());
                                        RosterEntry {
                                            name: p_name,
                                            tag: p_tag,
                                        }
                                    }
                                    _ => continue,
                                };
                                if !entry.name.is_empty() {
                                    map.insert(id, entry);
                                }
                            }
                        }
                    } else if let Some(id_val) = obj
                        .get("id")
                        .or_else(|| obj.get("account_id"))
                        .or_else(|| obj.get("accountID"))
                        .or_else(|| obj.get("accountId"))
                    {
                        // Format 4: Array of individual player items
                        let id = id_val
                            .as_i64()
                            .or_else(|| id_val.as_str().and_then(|s| s.parse::<i64>().ok()));
                        let name = obj
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        if let Some(id) = id
                            && !name.is_empty()
                        {
                            map.insert(id, RosterEntry { name, tag: tag.clone() });
                        }
                    }
                }
            }
        }
        serde_json::Value::Object(obj) => {
            // Format 2: Object with "replays", "teams", or "clans" array
            if let Some(replays) = obj
                .get("replays")
                .or_else(|| obj.get("teams"))
                .or_else(|| obj.get("clans"))
            {
                parse_roster_value(replays, map);
                return;
            }

            // Format 3: Map of "<account_id>": "Name" or { "name": "...", "tag": "..." }
            for (k, v) in obj {
                if let Ok(id) = k.parse::<i64>() {
                    match v {
                        serde_json::Value::String(name) => {
                            let n = name.trim().to_string();
                            if !n.is_empty() {
                                map.insert(id, RosterEntry { name: n, tag: None });
                            }
                        }
                        serde_json::Value::Object(p_obj) => {
                            let name = p_obj
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("")
                                .trim()
                                .to_string();
                            let tag = p_obj
                                .get("tag")
                                .or_else(|| p_obj.get("clan"))
                                .and_then(|t| t.as_str())
                                .map(clean_tag);
                            if !name.is_empty() {
                                map.insert(id, RosterEntry { name, tag });
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        _ => {}
    }
}

/// Normalizes clan/team tag by removing surrounding brackets and excess whitespace.
pub fn clean_tag(raw: &str) -> String {
    raw.trim().trim_start_matches('[').trim_end_matches(']').trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_wows_tournaments_replays_format() {
        let json_data = r#"
        [
          {
            "tag": "-TWA-",
            "replay": "https://wows-tournaments.com/api/matches/report/replay?id=19038",
            "players": {
              "1015954101": "PeytonTheGreat",
              "1024508681": "Yuzuriha_Matoi"
            }
          },
          {
            "tag": "[H-SR]",
            "replay": "https://wows-tournaments.com/api/matches/report/replay?id=19039",
            "players": {
              "1034068447": "TestPlayer"
            }
          }
        ]
        "#;

        let map = parse_roster_json(json_data).unwrap();
        assert_eq!(map.len(), 3);

        let p1 = map.get(&1015954101).unwrap();
        assert_eq!(p1.name, "PeytonTheGreat");
        assert_eq!(p1.tag.as_deref(), Some("-TWA-"));

        let p3 = map.get(&1034068447).unwrap();
        assert_eq!(p3.name, "TestPlayer");
        assert_eq!(p3.tag.as_deref(), Some("H-SR"));
    }

    #[test]
    fn test_parse_discord_payload_wrapper() {
        let json_data = r#"
        {
          "callbackUrl": "https://wows-tournaments.com/api/matches/set-render-message",
          "targetChannelId": "1095991216170090540",
          "replays": [
            {
              "tag": "RAIN",
              "players": { "1234567": "RainPlayer" }
            }
          ]
        }
        "#;

        let map = parse_roster_json(json_data).unwrap();
        assert_eq!(map.len(), 1);
        let p = map.get(&1234567).unwrap();
        assert_eq!(p.name, "RainPlayer");
        assert_eq!(p.tag.as_deref(), Some("RAIN"));
    }

    #[test]
    fn test_parse_flat_dictionary_format() {
        let json_data = r#"
        {
          "1001": "SimpleName",
          "1002": { "name": "DetailedPlayer", "tag": "CLAN" }
        }
        "#;

        let map = parse_roster_json(json_data).unwrap();
        assert_eq!(map.len(), 2);
        assert_eq!(map.get(&1001).unwrap().name, "SimpleName");
        assert_eq!(map.get(&1001).unwrap().tag, None);
        assert_eq!(map.get(&1002).unwrap().name, "DetailedPlayer");
        assert_eq!(map.get(&1002).unwrap().tag.as_deref(), Some("CLAN"));
    }

    #[test]
    fn test_parse_wows_tournaments_developer_actual_example() {
        // Includes trailing comma on line 28 ("3021443165": "hude_2023",) as provided by developer
        let json_data = r#"
{
  "callbackUrl": "https://wows-tournaments.com/api/matches/set-render-message?secret=49173218-c173-11f1-84f0-02420a001460",
  "targetChannelId": "1096586857300631622",
  "replays": [
    {
      "tag": "HIBRO",
      "replay": "https://wows-tournaments.com/api/matches/report/replay?id=17153",
      "players": {
        "3021443197": "Peterlov_kc",
        "3021443194": "Noctis_poi",
        "3021443189": "Asa_suzuko",
        "3021443199": "Salsace_Prpr42",
        "3021443192": "ghs2015",
        "3021443193": "Nanakusa_Nichika",
        "3021443191": "In_memory_of_W",
        "3021443195": "Paplet__",
        "3021443188": "Iroha_Rinne"
      }
    },
    {
      "tag": "-WZU-",
      "replay": "https://wows-tournaments.com/api/matches/report/replay?id=17393",
      "players": {
        "3021443170": "xknb66666",
        "3021443171": "xxyxzwm",
        "3021443164": "Fat_sheep06",
        "3021443167": "Stanley2317",
        "3021443163": "3525531750",
        "3021443166": "Theskyoflove",
        "3021443165": "hude_2023",
      }
    }
  ]
}
        "#;

        let map = parse_roster_json(json_data).unwrap();
        // 9 HIBRO players + 7 -WZU- players = 16 players
        assert_eq!(map.len(), 16);

        let p1 = map.get(&3021443197).unwrap();
        assert_eq!(p1.name, "Peterlov_kc");
        assert_eq!(p1.tag.as_deref(), Some("HIBRO"));

        let p2 = map.get(&3021443165).unwrap();
        assert_eq!(p2.name, "hude_2023");
        assert_eq!(p2.tag.as_deref(), Some("-WZU-"));

        let p3 = map.get(&3021443170).unwrap();
        assert_eq!(p3.name, "xknb66666");
        assert_eq!(p3.tag.as_deref(), Some("-WZU-"));
    }
}
