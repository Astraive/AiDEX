import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ComponentType,
  type ReactNode,
} from "react";
import {
  AppShell,
  dockMinViewport,
  NavRail,
  SIDEBAR_DEFAULT_WIDTH,
  TopBar,
  TopBarStatus,
} from "@aidex/ui/desktop";
import {
  BoxIcon,
  KeyIcon,
  LockIcon,
  PanelLeftIcon,
  SearchIcon,
  SettingsIcon,
  UsersIcon,
} from "@aidex/ui/icons";
import {
  Badge,
  hasModalLayer,
  IconButton,
  Menu,
  MenuItem,
  MenuPopup,
  MenuSeparator,
  MenuTrigger,
  Text,
} from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { useAppearance } from "../app/providers/appearance";
import { useNavigation } from "../app/providers/navigation";
import { usePreferences } from "../app/providers/preferences";
import { useToast } from "../app/providers/toast";
import { useAidexStatus } from "../hooks/useAidexStatus";
import { useKeyChord } from "../hooks/useKeyChord";
import { useMediaQuery } from "../hooks/useMediaQuery";
import { useModifierKeyLabel } from "../hooks/useModifierKeyLabel";
import { formatKeyChord } from "../lib/keyChord";
import { railIdFor, railItems, type RailId } from "../lib/navigation";
import { pageTitles, samplePages, screenKeyOf } from "../lib/routes";
import { ShellContext, type Shell } from "./shell";
import { QuickSearch } from "./QuickSearch";

type Props = {
  Sidebar: ComponentType;
  onNewSession: () => void;
  children: ReactNode;
};

/** Default of `AppPreferences.shortcuts.toggle_sidebar` (the same default Settings › Shortcuts lists). */
const DEFAULT_TOGGLE_SIDEBAR_SHORTCUT = "Mod+b";

const rail = railItems.map(({ id, label, Icon }) => ({
  id,
  label,
  icon: <Icon />,
}));

export function MainLayout({ Sidebar, onNewSession, children }: Props) {
  const { route, navigate } = useNavigation();
  const { settings } = usePreferences();
  const { appearance, ready: appearanceReady, unsupported, update } = useAppearance();
  const notify = useToast();
  const modifierLabel = useModifierKeyLabel();
  const [searchOpen, setSearchOpen] = useState(false);
  const searchRef = useRef<HTMLInputElement>(null);
  const hideSidebarRef = useRef<HTMLButtonElement>(null);
  const showSidebarRef = useRef<HTMLButtonElement>(null);
  const contentRef = useRef<HTMLElement>(null);
  const sidebarToggled = useRef(false);
  const titleFocusPending = useRef(false);
  const pageKey = screenKeyOf(route);
  // Compared against the current key rather than a first-mount flag, which StrictMode's double effect defeats.
  const previousPageKey = useRef(pageKey);

  // The sidebar docks while the window leaves main at least 640px beside it; narrower windows show it
  // as an overlay instead, unless auto-collapse is off.
  const sidebarWidth = appearance.sidebar_width ?? SIDEBAR_DEFAULT_WIDTH;
  const wideEnough = useMediaQuery(`(min-width: ${dockMinViewport(sidebarWidth)}px)`);
  const docked = !appearance.sidebar_auto_collapse || wideEnough;
  // What the user chose since the layout last changed; null follows the layout (open when docked).
  const [userOpen, setUserOpen] = useState<boolean | null>(null);
  const [dockedBefore, setDockedBefore] = useState(docked);
  if (dockedBefore !== docked) {
    setDockedBefore(docked);
    setUserOpen(null);
  }
  const sidebarOpen = userOpen ?? docked;
  const overlayOpen = !docked && sidebarOpen;

  const toggleChord = settings.shortcuts?.toggle_sidebar || DEFAULT_TOGGLE_SIDEBAR_SHORTCUT;
  const sidebarShortcut = formatKeyChord(toggleChord, modifierLabel);

  const setSidebar = useCallback((open: boolean) => {
    sidebarToggled.current = true;
    setUserOpen(open);
  }, []);
  const closeOverlay = useCallback((open: boolean) => setUserOpen(open), []);

  // Mod+B (or the saved chord) toggles the sidebar, except while a dialog has the screen.
  useKeyChord(toggleChord, () => {
    if (!hasModalLayer()) setSidebar(!sidebarOpen);
  });

  // The toggle that was pressed may have unmounted: opening a docked sidebar from the top bar puts
  // focus on the sidebar's own hide button. (AppShell returns focus to the top-bar toggle on close.)
  useEffect(() => {
    if (!sidebarToggled.current) return;
    sidebarToggled.current = false;
    const active = document.activeElement;
    if (sidebarOpen && docked && (!active || active === document.body)) hideSidebarRef.current?.focus();
  }, [sidebarOpen, docked]);

  // A new page starts at the top and closes the overlay sidebar.
  useEffect(() => {
    if (previousPageKey.current === pageKey) return;
    previousPageKey.current = pageKey;
    contentRef.current?.scrollTo({ top: 0 });
    titleFocusPending.current = true;
    if (!docked) setUserOpen(null);
  }, [pageKey, docked]);

  // Its title takes focus so the change is announced, once main is no longer behind the overlay.
  useEffect(() => {
    if (!titleFocusPending.current || overlayOpen) return;
    titleFocusPending.current = false;
    contentRef.current
      ?.querySelector<HTMLElement>("h1[tabindex='-1']")
      ?.focus();
  }, [pageKey, overlayOpen]);

  const shell = useMemo<Shell>(
    () => ({
      focusSearch: () => setSearchOpen(true),
      collapseSidebar: () => setSidebar(false),
      startNewSession: onNewSession,
      collapseButtonRef: hideSidebarRef,
      sidebarShortcut,
    }),
    [onNewSession, setSidebar, sidebarShortcut],
  );

  const selectRail = (id: RailId) => {
    const item = railItems.find((candidate) => candidate.id === id);
    if (item) navigate(item.route);
  };

  // An older Core cannot save the width, so the separator only shows once the appearance can be saved.
  const resizable = appearanceReady && !unsupported;
  const onSidebarResizeEnd = useCallback(
    (width: number) => void update({ sidebar_width: width }),
    [update],
  );
  const onSidebarResetWidth = useCallback(
    () => void update({ sidebar_width: null }),
    [update],
  );

  const lockDrive = () =>
    void call("vault.lock")
      .then(() => window.location.reload())
      .catch(() =>
        notify(
          "The vault could not be locked. Stop active tasks and try again.",
          "danger",
        ),
      );

  const header = (
    <TopBar
      start={
        <>
          {sidebarOpen && docked ? null : (
            <IconButton
              ref={showSidebarRef}
              label="Show sidebar"
              icon={<PanelLeftIcon />}
              shortcut={sidebarShortcut}
              aria-expanded={docked ? undefined : sidebarOpen}
              onClick={() => setSidebar(true)}
            />
          )}
          {route.page === "home" ? null : (
            <Text as="span" variant="label" tone="secondary">
              {pageTitles[route.page]}
            </Text>
          )}
          {samplePages.has(route.page) ? (
            <Badge tone="neutral">Sample data</Badge>
          ) : null}
        </>
      }
      center={null}
      end={
        <>
          <CoreIndicator />
          <IconButton
            size="sm"
            label="Search conversations and projects"
            icon={<SearchIcon />}
            onClick={() => setSearchOpen(true)}
          />
          <IconButton
            size="sm"
            label="Manage models"
            icon={<BoxIcon />}
            onClick={() => navigate({ page: "models" })}
          />
          <Menu>
            <MenuTrigger
              render={
                <IconButton
                  size="sm"
                  label="Account and settings"
                  icon={<UsersIcon />}
                />
              }
            />
            <MenuPopup align="end" aria-label="Account and settings">
              <MenuItem
                icon={<KeyIcon />}
                onClick={() => navigate({ page: "models", tab: "providers" })}
              >
                Provider accounts
              </MenuItem>
              <MenuItem
                icon={<SettingsIcon />}
                onClick={() => navigate({ page: "settings" })}
              >
                Preferences
              </MenuItem>
              <MenuSeparator />
              <MenuItem icon={<LockIcon />} onClick={lockDrive}>
                Lock drive
              </MenuItem>
            </MenuPopup>
          </Menu>
        </>
      }
    />
  );

  return (
    <ShellContext.Provider value={shell}>
      <AppShell
        rail={
          <NavRail
            label="Main navigation"
            items={rail}
            activeId={railIdFor(route)}
            onSelect={selectRail}
          />
        }
        sidebar={<Sidebar />}
        sidebarMode={docked ? "docked" : "overlay"}
        sidebarOpen={sidebarOpen}
        onSidebarOpenChange={closeOverlay}
        sidebarWidth={sidebarWidth}
        onSidebarResizeEnd={resizable ? onSidebarResizeEnd : undefined}
        onSidebarResetWidth={resizable ? onSidebarResetWidth : undefined}
        sidebarToggleRef={showSidebarRef}
        header={header}
        contentRef={contentRef}
      >
        {children}
      </AppShell>
      <QuickSearch
        open={searchOpen}
        onClose={() => setSearchOpen(false)}
        inputRef={searchRef}
      />
    </ShellContext.Provider>
  );
}

/** Live drive and AiDEX Core state for the top bar; the details open in a small panel. */
function CoreIndicator() {
  const { status, bridgeMissing, daemonRunning } = useAidexStatus();
  const label = bridgeMissing
    ? "AiDEX unavailable"
    : !status
      ? "Checking drive"
      : status.drive.state !== "resolved"
        ? "No drive"
        : daemonRunning
          ? "Drive connected"
          : "AiDEX Core stopped";
  return (
    <TopBarStatus
      label={label}
      tone={daemonRunning ? "success" : "neutral"}
      title="Drive connection"
    >
      {status?.drive.state === "resolved"
        ? status.drive.root
        : "No portable drive selected"}
    </TopBarStatus>
  );
}
