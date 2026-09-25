//! Timeline query semantics over immutable Provider display rows.

use serde_json::{Value, json};
use server_model::ErrorCode;

use crate::protocol::timeline::{Direction, FetchRequest, SearchRequest};
use crate::storage::timeline::Row;

pub(crate) fn fetch(
    request: &FetchRequest,
    epoch: &str,
    rows: &[Row],
    agent: &Value,
) -> Result<Value, ErrorCode> {
    let direction = request.direction.unwrap_or(if request.cursor.is_some() {
        Direction::After
    } else {
        Direction::Tail
    });
    if direction != Direction::Tail && request.cursor.is_none() {
        return Err(ErrorCode::InvalidMessage);
    }
    let stale = request
        .cursor
        .as_ref()
        .is_some_and(|cursor| cursor.epoch != epoch);
    let next = rows.last().map_or(1, |row| row.seq + 1);
    let gap = request
        .cursor
        .as_ref()
        .is_some_and(|cursor| !stale && cursor.seq >= next && next > 1);
    let reset = stale || gap;
    let cursor = request.cursor.as_ref().map_or(0, |cursor| cursor.seq);
    let start = if !reset && direction == Direction::After {
        rows.partition_point(|row| row.seq <= cursor)
    } else {
        0
    };
    let end = if !reset && direction == Direction::Before {
        rows.partition_point(|row| row.seq < cursor)
    } else {
        rows.len()
    };
    let limit = request.limit.unwrap_or(if direction == Direction::After {
        0
    } else {
        200
    });
    let limit = if limit == 0 {
        end.saturating_sub(start)
    } else {
        limit
    };
    let (start, end) = if direction == Direction::After && !reset {
        (start, end.min(start.saturating_add(limit)))
    } else {
        (start.max(end.saturating_sub(limit)), end)
    };
    let selected = &rows[start..end];
    let mut value = json!({"agentId":request.agent_id,"agent":agent,"direction":direction,
        "projection":request.projection,"epoch":epoch,"reset":reset,"staleCursor":stale,"gap":gap,
        "window":{"minSeq":rows.first().map_or(0, |row| row.seq),"maxSeq":next.saturating_sub(1),"nextSeq":next},
        "startCursor":selected.first().map(|row|json!({"epoch":epoch,"seq":row.seq})),
        "endCursor":selected.last().map(|row|json!({"epoch":epoch,"seq":row.seq})),
        "hasOlder":start>0,"hasNewer":end<rows.len(),
        "entries":selected.iter().map(Row::value).collect::<Vec<_>>(),"error":null});
    if let Some(merge) = request.merge_window {
        value["mergeWindow"] = json!(merge);
    }
    bounded(value)
}

pub(crate) fn search(
    request: &SearchRequest,
    epoch: &str,
    rows: &[Row],
) -> Result<Value, ErrorCode> {
    let query = normalize(&request.query);
    if query.len() > 4096 {
        return Err(ErrorCode::InvalidMessage);
    }
    let offset = request.cursor.unwrap_or(0);
    let messages = searchable(rows);
    let mut matching = messages
        .iter()
        .filter(|row| row.seq > offset as u64 && !query.is_empty())
        .filter_map(|row| {
            let role = role(row)?;
            row.entry.item["text"]
                .as_str()
                .filter(|text| normalize(text).contains(&query))
                .map(|_| json!({"seq":row.seq,"role":role}))
        });
    let locations: Vec<_> = matching.by_ref().take(200).collect();
    let next = matching
        .next()
        .and_then(|_| locations.last().map(|location| location["seq"].clone()));
    bounded(
        json!({"agentId":request.agent_id,"epoch":epoch,"locations":locations,"nextCursor":next,"error":null}),
    )
}

fn searchable(rows: &[Row]) -> Vec<Row> {
    let mut messages: Vec<Row> = Vec::new();
    let mut positions = std::collections::BTreeMap::new();
    for row in rows.iter().filter(|row| role(row).is_some()) {
        if let Some(index) = positions.get(&row.entry.key).copied() {
            let existing: &mut Row = &mut messages[index];
            if let (Value::String(text), Some(delta)) = (
                &mut existing.entry.item["text"],
                row.entry.item["text"].as_str(),
            ) {
                text.push_str(delta);
            }
        } else {
            positions.insert(row.entry.key.clone(), messages.len());
            messages.push(row.clone());
        }
    }
    messages
}

pub(crate) fn prompts(agent: &str, epoch: &str, rows: &[Row]) -> Result<Value, ErrorCode> {
    let prompts: Vec<_> = rows
        .iter()
        .filter(|row| role(row) == Some("user"))
        .map(|row| {
            let text = row.entry.item["text"].as_str().unwrap_or("");
            let preview = preview(text);
            json!({"seq":row.seq,"timestamp":row.entry.timestamp,"preview":preview})
        })
        .collect();
    bounded(json!({"agentId":agent,"epoch":epoch,"prompts":prompts,"error":null}))
}

fn preview(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.encode_utf16().count() <= 120 {
        return collapsed;
    }
    let mut units = 0;
    let mut preview: String = collapsed
        .chars()
        .take_while(|character| {
            units += character.len_utf16();
            units <= 119
        })
        .collect();
    preview.push('…');
    preview
}

fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn role(row: &Row) -> Option<&'static str> {
    match row.entry.item["type"].as_str() {
        Some("user_message") => Some("user"),
        Some("assistant_message") => Some("assistant"),
        _ => None,
    }
}

pub(crate) fn bounded(value: Value) -> Result<Value, ErrorCode> {
    if serde_json::to_vec(&value)
        .map_err(|_| ErrorCode::AgentIo)?
        .len()
        > 900 * 1024
    {
        return Err(ErrorCode::ResourceExhausted);
    }
    Ok(value)
}

#[cfg(test)]
mod tests;
