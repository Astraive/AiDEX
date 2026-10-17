import { createIcon } from "./createIcon";

export const SearchIcon = createIcon(
  "SearchIcon",
  <>
    <circle cx="11" cy="11" r="6.5" />
    <path d="m20 20-4.4-4.4" />
  </>,
);

export const PanelLeftIcon = createIcon(
  "PanelLeftIcon",
  <>
    <rect x="3.5" y="4.5" width="17" height="15" rx="2.5" />
    <path d="M9.5 4.5v15" />
  </>,
);

export const PlusCircleIcon = createIcon(
  "PlusCircleIcon",
  <>
    <circle cx="12" cy="12" r="8.5" />
    <path d="M12 8.5v7M8.5 12h7" />
  </>,
);

export const MoreHorizontalIcon = createIcon(
  "MoreHorizontalIcon",
  <>
    <circle cx="5.5" cy="12" r="1.25" fill="currentColor" stroke="none" />
    <circle cx="12" cy="12" r="1.25" fill="currentColor" stroke="none" />
    <circle cx="18.5" cy="12" r="1.25" fill="currentColor" stroke="none" />
  </>,
);

export const PaperclipIcon = createIcon(
  "PaperclipIcon",
  <path d="m20 11.5-7.8 7.8a5 5 0 0 1-7.07-7.07l8.13-8.13a3.33 3.33 0 0 1 4.71 4.71l-8.14 8.13a1.67 1.67 0 0 1-2.36-2.36l7.43-7.42" />,
);

export const ArrowUpIcon = createIcon(
  "ArrowUpIcon",
  <path d="M12 19V5M5.5 11.5 12 5l6.5 6.5" />,
);

export const ArrowRightIcon = createIcon(
  "ArrowRightIcon",
  <path d="M5 12h14M12.5 5.5 19 12l-6.5 6.5" />,
);

const eyeOutline = "M2.5 12C4.5 8 8 5.5 12 5.5s7.5 2.5 9.5 6.5c-2 4-5.5 6.5-9.5 6.5s-7.5-2.5-9.5-6.5Z";

export const EyeIcon = createIcon(
  "EyeIcon",
  <>
    <path d={eyeOutline} />
    <circle cx="12" cy="12" r="3" />
  </>,
);

export const EyeOffIcon = createIcon(
  "EyeOffIcon",
  <>
    <path d={eyeOutline} />
    <circle cx="12" cy="12" r="3" />
    <path d="m4 4 16 16" />
  </>,
);

export const CheckCircleIcon = createIcon(
  "CheckCircleIcon",
  <>
    <circle cx="12" cy="12" r="8.5" />
    <path d="m8.5 12.25 2.4 2.4 4.6-4.9" />
  </>,
);

/** Counter-clockwise arrow: undo a change or reset a value to its default. */
export const RotateCcwIcon = createIcon(
  "RotateCcwIcon",
  <>
    <path d="M4.5 12a7.5 7.5 0 1 0 2.2-5.3L4.5 9" />
    <path d="M4.5 4.5V9H9" />
  </>,
);
