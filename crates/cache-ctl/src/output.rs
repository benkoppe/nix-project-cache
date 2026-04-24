use std::io::Write;

use anyhow::Result;
use serde::Serialize;

pub fn print_json<T: Serialize + ?Sized>(writer: &mut impl Write, value: &T) -> Result<()> {
    writeln!(writer, "{}", serde_json::to_string_pretty(value)?)?;
    Ok(())
}

pub fn print_status_json(
    writer: &mut impl Write,
    status: &str,
    fields: impl IntoIterator<Item = (&'static str, serde_json::Value)>,
) -> Result<()> {
    let mut object = serde_json::Map::new();
    object.insert(
        "status".to_owned(),
        serde_json::Value::String(status.to_owned()),
    );

    for (key, value) in fields {
        object.insert(key.to_owned(), value);
    }

    print_json(writer, &serde_json::Value::Object(object))
}
