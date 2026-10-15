import { useCallback, useState } from "react";
import { HomePage } from "../features/home/HomePage";
import { ProviderCatalogProvider } from "../features/model-manager/catalog/ProviderCatalog";
import { useKeyChord } from "../hooks/useKeyChord";
import { MainLayout } from "../layouts/MainLayout";
import { screenKeyOf } from "../lib/routes";
import { DEFAULT_NEW_SESSION_SHORTCUT, DEFAULT_PROVIDERS_SHORTCUT } from "../lib/shortcuts";
import { AppearanceProvider } from "./providers/appearance";
import { NavigationProvider, useNavigation } from "./providers/navigation";
import { PreferencesProvider, usePreferences } from "./providers/preferences";
import { ToastProvider } from "./providers/toast";
import { homeSidebar, screens } from "./registry";

/** The signed-in workspace: providers plus the route-driven shell. */
export function Workspace() {
  return (
    <NavigationProvider initialRoute={{ page: "chat" }}>
      <PreferencesProvider>
        <AppearanceProvider>
          <ToastProvider>
            <ProviderCatalogProvider>
              <WorkspaceScreens />
            </ProviderCatalogProvider>
          </ToastProvider>
        </AppearanceProvider>
      </PreferencesProvider>
    </NavigationProvider>
  );
}

function WorkspaceScreens() {
  const { route, navigate } = useNavigation();
  const { setPendingPrompt, setPendingWorkspaceId, setPendingAttachments, settings } = usePreferences();
  // A new chat resets its draft and optional project context.
  const [sessionKey, setSessionKey] = useState(0);

  const startNewSession = useCallback(() => {
    setPendingPrompt(null);
    setPendingWorkspaceId(null);
    setPendingAttachments(null);
    navigate({ page: "chat" });
    setSessionKey((key) => key + 1);
  }, [navigate, setPendingPrompt, setPendingWorkspaceId, setPendingAttachments]);

  const openProviders = useCallback(
    () => navigate({ page: "models", tab: "providers" }),
    [navigate],
  );

  // Bound from the same preferences Settings → Shortcuts lists.
  useKeyChord(settings.shortcuts?.new_session || DEFAULT_NEW_SESSION_SHORTCUT, startNewSession);
  useKeyChord(settings.shortcuts?.providers || DEFAULT_PROVIDERS_SHORTCUT, openProviders);

  if (route.page === "home") {
    return (
      <MainLayout Sidebar={homeSidebar} onNewSession={startNewSession}>
        <HomePage key={sessionKey} autoFocus />
      </MainLayout>
    );
  }

  const { Page, Sidebar } = screens[route.page];
  return (
    <MainLayout Sidebar={Sidebar} onNewSession={startNewSession}>
      {/* Keyed by screen so local state resets when the same page opens another workspace, agent, or model. */}
      <Page
        key={`${screenKeyOf(route)}:${route.page === "chat" && !route.id ? sessionKey : 0}`}
      />
    </MainLayout>
  );
}
