//! cargo build -p vorkspace-plugin --example hello --profile native-dev
//! Install the resulting examples/hello executable from Vorkspace → Plugins.
use serde_json::json;
use vorkspace_plugin::{Manifest, VERSION};
fn main() {
    let manifest = Manifest {
        protocol_version: VERSION,
        id: "example.hello".into(),
        name: "Hello plugin".into(),
        version: "1.0.0".into(),
        capabilities: vec![],
        ui: true,
        connector: false,
    };
    let result = vorkspace_plugin::serve(manifest, |_, method, _, _| match method {
        "ui.document" => Ok(
            json!({"html":"<main><h1>Hello</h1><p>This interface belongs to an independent executable.</p><button id='hello'>Call the plugin</button><p id='reply' role='status'></p></main><script>document.querySelector('#hello').onclick=async()=>{try{document.querySelector('#reply').textContent=await vorkspace.call('hello')}catch(error){document.querySelector('#reply').textContent=error.message}}</script>"}),
        ),
        "hello" => Ok(json!("Hello from your Rust binary.")),
        _ => Err("Unknown method".into()),
    });
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1)
    }
}
