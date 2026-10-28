// Optional source-comparison tool. Compiles a temporary harness against the unchanged wheel crate.
// It creates no windows, actions or workspace files; review regenerated expectations before committing.
import { mkdtemp, mkdir, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";
const root = fileURLToPath(new URL("../../", import.meta.url));
execFileSync(
  process.execPath,
  [fileURLToPath(new URL("./check-demo.mjs", import.meta.url))],
  { stdio: "inherit" },
);
const temporary = await mkdtemp(join(tmpdir(), "radivine-wheel-reference-"));
const harness = String.raw`use radivine_wheel::*;
use serde_json::{json,Value};
fn menu(image:bool)->MenuTree {
  let first=if image {MenuNode::submenu("radivine.convert","Convert",vec![MenuNode::submenu("radivine.convert.image","Image",vec![MenuNode::leaf("radivine.convert.image.jpeg","JPEG"),MenuNode::leaf("radivine.convert.image.webp","WebP"),MenuNode::leaf("radivine.convert.image.advanced","Advanced…")])])} else {MenuNode::submenu("radivine.preview","File Preview",vec![MenuNode::leaf("radivine.preview.full","Full Preview"),MenuNode::leaf("radivine.preview.head","Head"),MenuNode::leaf("radivine.preview.tail","Tail")])};
  MenuTree::new(vec![first,MenuNode::leaf("radivine.copy","Copy"),MenuNode::leaf("radivine.properties","Properties"),MenuNode::submenu("radivine.custom","Custom",vec![MenuNode::leaf("custom.terminal","Open in Terminal"),MenuNode::submenu("custom.scripts","Scripts",vec![MenuNode::leaf("custom.scripts.sha256","Compute SHA-256"),MenuNode::leaf("custom.scripts.chmod","Make Executable")])])])
}
fn snapshot(s:&WheelSession,input:Value,now:u64,events:&[WheelEvent])->Value {
 let d=s.debug_state();
 json!({"at":now,"input":input,"path":d.open_chain.last().unwrap().as_slice(),"hover":d.hovered_item.map(|p|p.as_slice().to_vec()),"commit":events.iter().find_map(|e|if let WheelEvent::Commit(p)=e {Some(p.as_slice().to_vec())} else {None}),"wheels":d.wheels.iter().map(|w|json!({"center":{"x":w.center.x,"y":w.center.y},"inner":w.r_in,"outer":w.r_out,"spans":w.spans.iter().map(|sp|json!({"center":sp.center,"width":sp.a1-sp.a0})).collect::<Vec<_>>()})).collect::<Vec<_>>()})
}
fn main() {
 let mut cases=vec![];
 for layout in [WheelLayout::Branching,WheelLayout::Concentric] {
   let cfg=WheelConfig{layout,radius_root:[40.,128.],radius_child:[34.,108.],child_gap:20.,dwell_ms:110,grace_ms:180,hysteresis_deg:3.5,..WheelConfig::default()};
   for image in [false,true] {
     let mut s=WheelSession::new(Some(menu(image)),Point::new(450.,350.),Viewport::new(900.,800.,1.),InputMode::Pointer,&cfg,0);
     let mut trace=vec![];
     for (now,nav,key) in [(10,NavKey::Digit(1),"1"),(20,NavKey::Back,"Backspace"),(30,NavKey::Digit(1),"1"),(40,NavKey::Digit(1),"1")] {
       let mut ev=vec![];s.handle(WheelInput::Nav(nav),now,&mut ev); trace.push(snapshot(&s,json!({"type":"key","key":key}),now,&ev));
     }
     cases.push(json!({"name":"keyboard-geometry","layout":layout.as_str(),"context":if image {"image"} else {"text"},"trace":trace}));
   }
   let mut s=WheelSession::new(Some(menu(false)),Point::new(450.,350.),Viewport::new(900.,800.,1.),InputMode::Pointer,&cfg,0);
   let mut trace=vec![];
   for (now,input) in [(0,Some(Point::new(450.,280.))),(109,None),(110,None),(120,Some(Point::new(520.,350.))),(299,None),(300,None)] {
     let mut ev=vec![];
     let event=if let Some(p)=input {s.handle(WheelInput::PointerMoved(p),now,&mut ev);json!({"type":"move","point":{"x":p.x,"y":p.y}})} else {s.tick(now,&mut ev);json!({"type":"tick"})};
     trace.push(snapshot(&s,event,now,&ev));
   }
   cases.push(json!({"name":"dwell-and-inward-sibling","layout":layout.as_str(),"context":"text","trace":trace}));
 }
 println!("{}",serde_json::to_string_pretty(&json!({"producer":"Unmodified radivine-wheel v0.3.0, temporary standalone harness; no native windows or actions executed","sourceRevision":"1aa009a08bab66646898016f1ba953051eac46f3","cases":cases})).unwrap());
}
`;
try {
  await mkdir(join(temporary, "src"));
  await writeFile(join(temporary, "src/main.rs"), harness);
  await writeFile(
    join(temporary, "Cargo.toml"),
    `[package]
name = "radivine-website-reference"
version = "0.0.0"
edition = "2024"
[dependencies]
radivine-wheel = { path = ${JSON.stringify(join(root, "crates/radivine-wheel"))} }
serde_json = "=1.0.154"
`,
  );
  const json = execFileSync(
    process.env.CARGO || "cargo",
    [
      "run",
      "--quiet",
      "--offline",
      "--manifest-path",
      join(temporary, "Cargo.toml"),
    ],
    { encoding: "utf8", maxBuffer: 4 * 1024 * 1024 },
  );
  await writeFile(
    new URL("../tests/fixtures/demo-native-traces.json", import.meta.url),
    JSON.stringify(JSON.parse(json), null, 2) + "\n",
  );
  console.log(
    "Regenerated six Rust engine traces. Review the diff, format the JSON and run npm run test:demo.",
  );
} finally {
  await rm(temporary, { recursive: true, force: true });
}
