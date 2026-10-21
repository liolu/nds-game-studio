fn main() {
    // Le manifeste Windows n'a de sens que pour une cible Windows.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("app.rc", embed_resource::NONE);
    }
}
