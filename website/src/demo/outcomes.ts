import type { Action } from "./contexts.ts";
export function outcome(action: Action): string {
  if (action.id.startsWith("custom."))
    return `Simulation: ${action.label} is a logging-only example in the default desktop menu. Configure a custom command to perform this operation. Nothing ran here.`;
  if (action.id.endsWith(".advanced"))
    return "Simulation: the desktop app would open Advanced Convert to choose conversion settings. No job was created here.";
  if (action.id.startsWith("radivine.convert."))
    return `Simulation: a ${action.label} conversion job would be queued for the desktop worker. Quick Convert is asynchronous; no file was converted here.`;
  if (action.id.startsWith("radivine.preview."))
    return `Simulation: the desktop app would open ${action.label}${action.label === "Head" ? " (first 50 lines)" : action.label === "Tail" ? " (last 50 lines)" : ""} in the read-only viewer. No local file was opened.`;
  if (action.id === "radivine.copy")
    return "Simulation: the desktop app would offer the selected file URIs to the X11 clipboard. Your browser clipboard was not changed.";
  return "Simulation: with a supported file manager available, the desktop app would request file properties. No native window was opened.";
}
