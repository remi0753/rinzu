use crate::source_edit::ReplaceChecked;

pub fn source(process_main: i32) -> String {
    let source = include_str!("main.rinzu")
        .replace_checked("let root = [\"Worker heap\", \"no Main heap access\"];", "let root = [\"Worker heap\", \"no Main heap access\"];let workerDelay=dispatcher.workerDelay(80);")
        .replace_checked("dispatcher.done(result);", "let flowDelay=dispatcher.delay(80);dispatcher.done(result);")
        .replace_checked("dispatcher.nested();", "dispatcher.nested();try{let packet=dispatcher.readSize(\"data/proof.bin\");let observedPacket=observePacket(packet);}catch packetErrors{panic(\"packetFailure\");}let callbackRoot=[\"callback rooted across long native\"];let callbackDelay=dispatcher.delay(120);if callbackRoot!=[\"callback rooted across long native\"]{panic(\"lostCallbackRoot\");}")
        .replace_checked("dispatcher.reset(0)", &format!("dispatcher.reset({process_main})"));
    format!(
        "{source}\nasync fn observePacket(packet:Future<Int>)->Int {{try{{return await packet;}}catch errors{{panic(\"unexpected failure\"); return 0;}}}}\n"
    )
}
pub fn main_ready_source(process_main: i32) -> String {
    source(process_main)
        .replace_checked(
            "async fn flow(seed: CInt) -> CInt {",
            r#"
async fn holdReady(packet:Future<Int>)->Int {
    let value=await packet;
    let longReadyDelay=dispatcher.delay(1200);
    return value;
}
async fn observeReady(packet:Future<Int>)->Int {
    let root=["ready Main root", "wait after packet reception"];
    let value=await packet;
    if root!=["ready Main root", "wait after packet reception"]{panic("lostReadyRoot");}
    return value;
}
async fn delayedPacket()->Int {
    try {
        await dispatcher.timer(50);
        return await dispatcher.readSize("data/proof.bin");
    } catch errors {panic("unexpected failure"); return 0;}
}
async fn flow(seed:CInt)->CInt {"#,
        )
        .replace_checked(
            "let first = await dispatcher.readSize(\"data/proof.bin\");",
            r#"
    let packet=delayedPacket();
    let held=holdReady(packet);
    let observed=observeReady(packet);
    let first=await packet;
    let heldValue=await held;
    let observedValue=await observed;
    if heldValue!=8193||observedValue!=8193{panic("wrongReady");}"#,
        )
}
