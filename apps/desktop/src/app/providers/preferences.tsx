import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { DEFAULT_ACCENT, applyAccent } from "@aidex/ui/themes";
import { call } from "@aidex/client/rpc";
import { toAidexError } from "@aidex/client/errors";
import type { AppPreferences } from "@aidex/client/dto/preferences";
import type { DraftAttachment } from "../../features/chat/attachments/attachmentDraft";
import { useRpc } from "../../hooks/useRpc";

type Preferences = {
  modelId: string;
  setModelId: (id: string) => void;
  accent: string;
  setAccent: (color: string) => void;
  workspaceId: string;
  setWorkspaceId: (id: string) => void;
  pendingPrompt: string | null;
  setPendingPrompt: (prompt: string | null) => void;
  pendingWorkspaceId: string | null;
  setPendingWorkspaceId: (id: string | null) => void;
  /** Attachments handed from Home to the new chat with `pendingPrompt` (in memory only). */
  pendingAttachments: readonly DraftAttachment[] | null;
  setPendingAttachments: (items: readonly DraftAttachment[] | null) => void;
  settings: AppPreferences;
  updateSettings: (patch: Partial<AppPreferences>) => Promise<void>;
  loaded: boolean;
  error: string | null;
};
const defaults: AppPreferences = {
  accent: DEFAULT_ACCENT,
  theme: "dark",
  density: "comfortable",
  reduce_motion: false,
  keyboard_hints: true,
  model_id: "",
  workspace_id: "",
  permission_mode: null,
  instructions: "",
  fallback_model: null,
  max_steps: 16,
  max_tokens: 4096,
  shortcuts: {
    new_session: "Mod+k",
    search: "Mod+p",
    toggle_sidebar: "Mod+b",
    providers: "Mod+Shift+p",
  },
};
const PreferencesContext = createContext<Preferences | null>(null);

export function PreferencesProvider({ children }: { children: ReactNode }) {
  const query = useRpc("settings.get");
  const [settings, setSettings] = useState<AppPreferences>(defaults);
  const [error, setError] = useState<string | null>(null);
  const [pendingPrompt, setPendingPrompt] = useState<string | null>(null);
  const [pendingWorkspaceId, setPendingWorkspaceId] = useState<string | null>(
    null,
  );
  const [pendingAttachments, setPendingAttachments] = useState<
    readonly DraftAttachment[] | null
  >(null);
  useEffect(() => {
    if (query.data) setSettings(query.data);
  }, [query.data]);
  const updateSettings = useCallback(async (patch: Partial<AppPreferences>) => {
    setError(null);
    try {
      const saved = await call("settings.set", { patch });
      setSettings(saved);
    } catch (caught) {
      const message = toAidexError(caught).message;
      setError(message);
      throw caught;
    }
  }, []);
  const updateQuietly = useCallback(
    (patch: Partial<AppPreferences>) => {
      void updateSettings(patch).catch(() => undefined);
    },
    [updateSettings],
  );
  useEffect(() => {
    applyAccent(settings.accent);
    const root = document.documentElement;
    const media = window.matchMedia("(prefers-color-scheme: light)");
    const applyTheme = () => {
      root.dataset.theme =
        settings.theme === "system"
          ? media.matches
            ? "light"
            : "dark"
          : settings.theme;
    };
    applyTheme();
    media.addEventListener("change", applyTheme);
    root.dataset.density = settings.density;
    root.dataset.reduceMotion = String(settings.reduce_motion);
    root.dataset.keyboardHints = String(settings.keyboard_hints);
    return () => media.removeEventListener("change", applyTheme);
  }, [
    settings.accent,
    settings.theme,
    settings.density,
    settings.reduce_motion,
    settings.keyboard_hints,
  ]);
  const value = useMemo<Preferences>(
    () => ({
      modelId: settings.model_id,
      setModelId: (model_id) => updateQuietly({ model_id }),
      accent: settings.accent,
      setAccent: (accent) => updateQuietly({ accent }),
      workspaceId: settings.workspace_id,
      setWorkspaceId: (workspace_id) => updateQuietly({ workspace_id }),
      pendingPrompt,
      setPendingPrompt,
      pendingWorkspaceId,
      setPendingWorkspaceId,
      pendingAttachments,
      setPendingAttachments,
      settings,
      updateSettings,
      loaded: !!query.data,
      error: error ?? query.error?.message ?? null,
    }),
    [
      settings,
      updateQuietly,
      updateSettings,
      pendingPrompt,
      pendingWorkspaceId,
      pendingAttachments,
      query.data,
      query.error,
      error,
    ],
  );
  return (
    <PreferencesContext.Provider value={value}>
      {children}
    </PreferencesContext.Provider>
  );
}
export function usePreferences(): Preferences {
  const preferences = useContext(PreferencesContext);
  if (!preferences)
    throw new Error("usePreferences must be used inside PreferencesProvider");
  return preferences;
}
