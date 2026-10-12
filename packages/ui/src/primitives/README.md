# UI primitives

Small accessible controls and layout primitives shared across frontends:

- Actions: `Button`, `IconButton`, `Kbd`.
- Overlays: `Popover` (anchored, non-modal, portalled).
- Form fields: `TextField`, `SearchField`, `PasswordField`, `TextArea`, `Select`, `Checkbox`, `Switch`, `SegmentedControl`, `ChoiceCardGroup`, `SwatchPicker`. Labels, hints, and errors share `field.module.css`.
- Navigation: `Tabs` and `TabPanel`.
- Display: `Avatar`, `Badge`, `StatusDot`, `StatusIndicator`, `IconBadge`, `ProgressBar`, `Divider`, `Card`.
- Layout and type: `Stack`, `Grid`, `Split`, `Container`, `Heading`, `Text`. `Stack` and `Grid` share the token gap scale in `gap.module.css`.

Each component keeps its styles in a sibling CSS Module.
