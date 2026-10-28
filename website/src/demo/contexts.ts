/** Reviewed v0.3.0 fixtures; no filesystem or runtime capability detection. */
export type Availability = "enabled" | "disabled" | "pending";
export interface Action {
  id: string;
  label: string;
  short: string;
  state: Availability;
  reason?: string;
  children?: Action[];
}
export type ContextId = "text" | "image" | "multiple" | "checking";
export const contexts: {
  id: ContextId;
  label: string;
  file: string;
  detail: string;
}[] = [
  {
    id: "text",
    label: "Text file",
    file: "field-notes.txt",
    detail: "One readable text file · Full, Head and Tail preview",
  },
  {
    id: "image",
    label: "PNG image",
    file: "fern.png",
    detail:
      "One static PNG · JPEG / WebP conversion; PNG is already the source format",
  },
  {
    id: "multiple",
    label: "Two text files",
    file: "field-notes.txt + todo.txt",
    detail: "Two readable text files · Preview requires a single file",
  },
  {
    id: "checking",
    label: "Checking a text file",
    file: "inspection.txt",
    detail:
      "Illustrative inspection: preview is pending for 1.8 seconds after opening",
  },
];
const leaf = (id: string, label: string, short = label): Action => ({
  id,
  label,
  short,
  state: "enabled",
});
const branch = (
  id: string,
  label: string,
  children: Action[],
  short = label,
): Action => ({ ...leaf(id, label, short), children });
export function menuFor(context: ContextId, inspected = false): Action[] {
  const preview = branch(
    "radivine.preview",
    "File Preview",
    [
      leaf("radivine.preview.full", "Full Preview", "Full"),
      leaf("radivine.preview.head", "Head"),
      leaf("radivine.preview.tail", "Tail"),
    ],
    "Preview",
  );
  // Registry keeps submenus navigable; only leaves receive enablement reasons.
  for (const item of preview.children!) {
    if (context === "multiple")
      Object.assign(item, {
        state: "disabled",
        reason: "Select a single file to preview",
      });
    if (context === "checking" && !inspected)
      Object.assign(item, { state: "pending", reason: "Checking…" });
  }
  return [
    context === "image"
      ? branch("radivine.convert", "Convert", [
          branch("radivine.convert.image", "Image", [
            leaf("radivine.convert.image.jpeg", "JPEG"),
            leaf("radivine.convert.image.webp", "WebP"),
            leaf("radivine.convert.image.advanced", "Advanced…", "Advanced"),
          ]),
        ])
      : preview,
    leaf("radivine.copy", "Copy"),
    leaf("radivine.properties", "Properties", "Info"),
    branch("radivine.custom", "Custom", [
      leaf("custom.terminal", "Open in Terminal", "Terminal"),
      branch("custom.scripts", "Scripts", [
        leaf("custom.scripts.sha256", "Compute SHA-256", "SHA-256"),
        leaf("custom.scripts.chmod", "Make Executable", "Executable"),
      ]),
    ]),
  ];
}
export function itemsAt(menu: Action[], path: number[]): Action[] {
  let items = menu;
  for (const index of path) items = items[index]?.children ?? [];
  return items;
}
export function actionAt(menu: Action[], path: number[]): Action | undefined {
  return itemsAt(menu, path.slice(0, -1))[path.at(-1) ?? -1];
}
