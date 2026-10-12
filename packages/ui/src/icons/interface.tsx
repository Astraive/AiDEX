import { createIcon } from "./createIcon";

export const PlusIcon = createIcon("PlusIcon", <path d="M12 5v14M5 12h14" />);

export const MinusIcon = createIcon("MinusIcon", <path d="M5 12h14" />);

export const CheckIcon = createIcon("CheckIcon", <path d="m5 12.5 4.5 4.5L19 7" />);

export const XIcon = createIcon("XIcon", <path d="M6 6l12 12M18 6 6 18" />);

export const ChevronDownIcon = createIcon("ChevronDownIcon", <path d="m6 9 6 6 6-6" />);

export const ChevronUpIcon = createIcon("ChevronUpIcon", <path d="m6 15 6-6 6 6" />);

export const ChevronRightIcon = createIcon("ChevronRightIcon", <path d="m9 6 6 6-6 6" />);

export const ChevronLeftIcon = createIcon("ChevronLeftIcon", <path d="m15 6-6 6 6 6" />);

export const ArrowLeftIcon = createIcon("ArrowLeftIcon", <path d="M19 12H5M11.5 5.5 5 12l6.5 6.5" />);

export const RefreshIcon = createIcon(
  "RefreshIcon",
  <path d="M20 11.5A8 8 0 0 0 6.3 6.3L4 8.5M4 4v4.5h4.5M4 12.5a8 8 0 0 0 13.7 5.2l2.3-2.2M20 20v-4.5h-4.5" />,
);

export const TrashIcon = createIcon(
  "TrashIcon",
  <path d="M4.5 7h15M9.5 7V5a1.5 1.5 0 0 1 1.5-1.5h2A1.5 1.5 0 0 1 14.5 5v2M6.5 7l.8 12a2 2 0 0 0 2 1.9h5.4a2 2 0 0 0 2-1.9l.8-12M10 11v6M14 11v6" />,
);

export const PencilIcon = createIcon(
  "PencilIcon",
  <path d="M16.5 4.5a2.12 2.12 0 0 1 3 3L8.5 18.5 4 20l1.5-4.5ZM14.5 6.5l3 3" />,
);

export const CopyIcon = createIcon(
  "CopyIcon",
  <>
    <rect x="8.5" y="8.5" width="12" height="12" rx="2" />
    <path d="M15.5 8.5V5.5a2 2 0 0 0-2-2h-8a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h3" />
  </>,
);

export const ExternalLinkIcon = createIcon(
  "ExternalLinkIcon",
  <path d="M14 4h6v6M20 4l-9 9M18 14v4a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h4" />,
);

export const FilterIcon = createIcon("FilterIcon", <path d="M4 5h16l-6.2 7.4V19l-3.6-1.8v-4.8Z" />);

export const DownloadIcon = createIcon("DownloadIcon", <path d="M12 4v11M7 10.5l5 5 5-5M5 19.5h14" />);

export const UploadIcon = createIcon("UploadIcon", <path d="M12 15.5V4.5M7 9l5-5 5 5M5 19.5h14" />);

export const PlayIcon = createIcon("PlayIcon", <path d="M7.5 5.5v13l11-6.5Z" />);

export const PauseIcon = createIcon(
  "PauseIcon",
  <>
    <rect x="6.5" y="5" width="3.5" height="14" rx="1" />
    <rect x="14" y="5" width="3.5" height="14" rx="1" />
  </>,
);

export const StopIcon = createIcon("StopIcon", <rect x="6" y="6" width="12" height="12" rx="2" />);

/** Three-quarter ring; spin it with CSS for an indeterminate state. */
export const LoaderIcon = createIcon("LoaderIcon", <path d="M12 3.5a8.5 8.5 0 1 0 8.5 8.5" />);

export const InfoIcon = createIcon(
  "InfoIcon",
  <>
    <circle cx="12" cy="12" r="8.5" />
    <path d="M12 11v5M12 7.75h.01" />
  </>,
);

export const AlertTriangleIcon = createIcon(
  "AlertTriangleIcon",
  <path d="M10.3 4.2 2.9 17.5a2 2 0 0 0 1.7 3h14.8a2 2 0 0 0 1.7-3L13.7 4.2a2 2 0 0 0-3.4 0ZM12 9.5v4M12 17h.01" />,
);

export const XCircleIcon = createIcon(
  "XCircleIcon",
  <>
    <circle cx="12" cy="12" r="8.5" />
    <path d="m9 9 6 6M15 9l-6 6" />
  </>,
);

export const CircleIcon = createIcon("CircleIcon", <circle cx="12" cy="12" r="8.5" />);

export const BellIcon = createIcon(
  "BellIcon",
  <path d="M6 9.5a6 6 0 0 1 12 0c0 6 2.5 7.5 2.5 7.5h-17S6 15.5 6 9.5ZM10 20a2.2 2.2 0 0 0 4 0" />,
);

export const SlidersIcon = createIcon(
  "SlidersIcon",
  <>
    <path d="M4 7h9M17 7h3M4 17h3M11 17h9" />
    <circle cx="15" cy="7" r="2" />
    <circle cx="9" cy="17" r="2" />
  </>,
);

export const KeyboardIcon = createIcon(
  "KeyboardIcon",
  <>
    <rect x="2.5" y="6" width="19" height="12" rx="2" />
    <path d="M6.5 10h.01M10 10h.01M13.5 10h.01M17 10h.01M7.5 14h9" />
  </>,
);

export const PaletteIcon = createIcon(
  "PaletteIcon",
  <>
    <path d="M12 3.5a8.5 8.5 0 0 0 0 17c1.1 0 1.8-.8 1.8-1.7 0-.5-.2-.9-.5-1.2-.3-.3-.5-.7-.5-1.2 0-.9.8-1.7 1.8-1.7h2.1a3.8 3.8 0 0 0 3.8-3.8c0-4-3.8-7.4-8.5-7.4Z" />
    <circle cx="7.5" cy="11.5" r="1" fill="currentColor" stroke="none" />
    <circle cx="9.5" cy="7.5" r="1" fill="currentColor" stroke="none" />
    <circle cx="14.5" cy="7.5" r="1" fill="currentColor" stroke="none" />
  </>,
);

export const PinIcon = createIcon(
  "PinIcon",
  <path d="M9 3.5h6M10 3.5v5.5l-3.5 3.5V14h11v-1.5L14 9V3.5M12 14v6.5" />,
);

export const ListIcon = createIcon("ListIcon", <path d="M8.5 6H20M8.5 12H20M8.5 18H20M4 6h.01M4 12h.01M4 18h.01" />);

export const UserIcon = createIcon(
  "UserIcon",
  <>
    <circle cx="12" cy="8" r="3.5" />
    <path d="M5 20a7 7 0 0 1 14 0" />
  </>,
);

const STAR_PATH = "M12 3.4 14.3 9.4l6.45.35-5.04 4.06 1.7 6.23L12 16.5l-5.41 3.54 1.7-6.23-5.04-4.06 6.45-.35Z";

/** Outline star, e.g. for adding an item to favorites. */
export const StarIcon = createIcon("StarIcon", <path d={STAR_PATH} />);

/** Filled star marking a favorite. */
export const StarFilledIcon = createIcon("StarFilledIcon", <path d={STAR_PATH} fill="currentColor" />);

/** Picture placeholder, e.g. for an image that is linked rather than loaded. */
export const ImageIcon = createIcon(
  "ImageIcon",
  <>
    <rect x="3.5" y="4.5" width="17" height="15" rx="2.5" />
    <circle cx="9" cy="10" r="1.75" />
    <path d="m20.5 15.5-4.25-4.25a1.5 1.5 0 0 0-2.12 0L5.5 19.5" />
  </>,
);

/** Magnifier with a plus, e.g. to zoom an image in. */
export const ZoomInIcon = createIcon(
  "ZoomInIcon",
  <>
    <circle cx="11" cy="11" r="6.5" />
    <path d="m20 20-4.4-4.4M11 8.25v5.5M8.25 11h5.5" />
  </>,
);

/** Magnifier with a minus, e.g. to fit a zoomed image back into view. */
export const ZoomOutIcon = createIcon(
  "ZoomOutIcon",
  <>
    <circle cx="11" cy="11" r="6.5" />
    <path d="m20 20-4.4-4.4M8.25 11h5.5" />
  </>,
);

/** Two opposing arrows, e.g. a conversation that switched mode or continued elsewhere. */
export const ArrowRightLeftIcon = createIcon(
  "ArrowRightLeftIcon",
  <path d="M4.5 8h15M15.5 4l4 4-4 4M19.5 16h-15M8.5 12l-4 4 4 4" />,
);
