# Shared components

Product-level components shared across desktop, installer, and mobile:

- Root: `Brand`, `Composer` (and its attach menu, `ComposerAttachMenu`, on the `Menu` primitive), `Dialog` (Base UI), `MediaItem`, `InfoCard`, `PageHeader` and `SectionHeader`, and the toasts on Base UI Toast (`ToastProvider` with `useToastManager`, and `ToastRegion` for an owner-kept list).
- `attachments/`: the composer's attachment tray (`AttachmentTray`), sent-message attachments (`MessageAttachments`), the image viewer (`ImageLightbox`), the page-level drop target (`FileDropZone`), the window drop guard (`useFileDropGuard`), and the drag guard for modals (`containFileDrag`, used by `Dialog` and `ImageLightbox`). Paste, drag, size, and data-URL rules live in `model.ts`; the shared drag-over state in `useFileDrop.ts`.
- `ModelPicker/`: the provider-grouped model picker (`ModelPicker`, `ModelPickerTrigger`). Pure search ranking lives in `search.ts` and view derivation in `view.ts`, so both stay testable without a DOM.
- `CommandMenu/`: the composer's `$` skills and `/` command popup (`CommandMenu`). Option ids, field ARIA, match highlighting, and the highlight and key logic (`resolveCommandMenuActive`, `commandMenuKeyAction`) live in `model.ts`.
- `data/`: lists, key-value rows, setting rows and settings sections, provider rows and tiles, empty states, stat tiles, meters, charts, tables, entity cards, artifact cards, and plan cards (plan title and preview logic in `planText.ts`).
- `content/`: markdown (pure parser in `parseMarkdown.ts`), `$skill` chips (token grammar in `skillTokens.ts`), work-log rows and groups, web search results, plan steps, activity logs, task graphs, diffs, code and terminal blocks, chat messages, transcript dividers, file trees, timelines, and the decorative pairing-code pattern.

URL checks for untrusted links (`safeExternalUrl`, `urlHost`) live in `../utils/url.ts`; byte and date formatting in `../utils/format.ts`.
