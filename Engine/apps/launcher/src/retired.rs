pub fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    use serde::Deserialize;
    serde::de::IgnoredAny::deserialize(d)?;
    Ok(true)
}

pub fn objects_note(present: bool) -> Option<&'static str> {
    present.then_some(
        "settings.json: `objects` (Placed objects) is not read any more: the placed objects are always drawn; the \
         member is dropped at the next save",
    )
}
