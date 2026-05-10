// napi-rs build hook — emits the Node-API headers + linker flags for
// the host platform. Required for any napi-rs cdylib build.
fn main() {
    napi_build::setup();
}
