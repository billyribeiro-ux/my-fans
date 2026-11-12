use super::*;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Default, Deserialize)]
#[serde(transparent)]
pub struct PageQuery(BTreeMap<String, String>);

#[derive(Default)]
pub struct Cursor {
    date: Option<chrono::DateTime<chrono::Utc>>,
    id: Option<Uuid>,
}

impl PageQuery {
    pub fn validate(&self, keys: &[&str]) -> Result<()> {
        if self.0.keys().any(|key| !keys.contains(&key.as_str())) {
            return Err(bad("Invalid pagination query."));
        }
        for key in keys {
            self.cursor(key)?;
        }
        Ok(())
    }
    pub fn cursor(&self, key: &str) -> Result<Cursor> {
        let Some(value) = self.0.get(key) else {
            return Ok(Cursor::default());
        };
        if value.len() > 90 {
            return Err(bad("Invalid pagination cursor."));
        }
        let (date, id) = value
            .split_once('|')
            .ok_or(bad("Invalid pagination cursor."))?;
        Ok(Cursor {
            date: Some(
                date.parse()
                    .map_err(|_| bad("Invalid pagination cursor."))?,
            ),
            id: Some(id.parse().map_err(|_| bad("Invalid pagination cursor."))?),
        })
    }
}

// Selects and column names come exclusively from server code. All request values are bound.
// The UUID tie breaker prevents duplicates and omissions when timestamps are identical.
pub async fn read<'e, E: sqlx::Executor<'e, Database = sqlx::Postgres>>(
    db: E,
    select: &'static str,
    viewer: Option<Uuid>,
    cursor: Cursor,
    date_field: &'static str,
    oldest_first: bool,
) -> Result<(Value, Option<String>)> {
    read_scoped(db, select, viewer, cursor, date_field, oldest_first, None).await
}

pub async fn read_scoped<'e, E: sqlx::Executor<'e, Database = sqlx::Postgres>>(
    db: E,
    select: &'static str,
    viewer: Option<Uuid>,
    cursor: Cursor,
    date_field: &'static str,
    oldest_first: bool,
    resource: Option<Uuid>,
) -> Result<(Value, Option<String>)> {
    let (comparison, direction) = if oldest_first {
        (">", "ASC")
    } else {
        ("<", "DESC")
    };
    let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("WITH viewer AS (SELECT ");
    query
        .push_bind(viewer)
        .push("::uuid AS id,")
        .push_bind(resource)
        .push("::uuid AS resource) SELECT COALESCE(jsonb_agg(to_jsonb(page) ORDER BY page.")
        .push(date_field)
        .push(" ")
        .push(direction)
        .push(",page.id ")
        .push(direction)
        .push("),'[]') FROM (SELECT x.* FROM (")
        .push(select)
        .push(") x WHERE (")
        .push_bind(cursor.date)
        .push("::timestamptz IS NULL OR (x.")
        .push(date_field)
        .push(",x.id)")
        .push(comparison)
        .push("(")
        .push_bind(cursor.date)
        .push(",")
        .push_bind(cursor.id)
        .push("::uuid)) ORDER BY x.")
        .push(date_field)
        .push(" ")
        .push(direction)
        .push(",x.id ")
        .push(direction)
        .push(" LIMIT 51) page");
    // The SELECT uses $1 for the viewer and $2 for the optional resource. Cursor parameters
    // follow these CTE bindings, so caller SQL never contains request-interpolated fragments.
    let mut rows: Value = query.build_query_scalar().fetch_one(db).await?;
    let array = rows
        .as_array_mut()
        .ok_or(bad("Invalid pagination result."))?;
    let more = array.len() > 50;
    array.truncate(50);
    let next = if more {
        let last = array.last().ok_or(bad("Invalid pagination result."))?;
        Some(format!(
            "{}|{}",
            last[date_field]
                .as_str()
                .ok_or(bad("Invalid pagination result."))?,
            last["id"]
                .as_str()
                .ok_or(bad("Invalid pagination result."))?
        ))
    } else {
        None
    };
    Ok((rows, next))
}
