//! Every scene drawn and every play run, from the same scenes dylib the window loads:
//! `smoke::<scene>` fails on an error or a panic, `play::<scene>::<name>` on a failed play.

gallery::test_main!(gallery::Renderer::Wgpu);
