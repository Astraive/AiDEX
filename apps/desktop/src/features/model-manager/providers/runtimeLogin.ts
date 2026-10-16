// Rules of a managed runtime sign-in (`runtime.login.*`) that do not need React.
import type { RuntimeLoginStatus } from "@aidex/client/dto/runtime";

/**
 * Whether a sign-in page still has to be opened: the flow is pending, reported its address, and was
 * not opened yet (`openedFor` is the flow already opened, if any). AiDEX opens it through `web.open`,
 * outside the sandbox job that runs the runtime's CLI, so ending the sign-in never closes the browser.
 */
export function shouldOpenSignInPage(
  openedFor: string | null,
  status: Pick<RuntimeLoginStatus, "state" | "flow_id" | "authorization_url">,
): boolean {
  return status.state === "pending" && !!status.authorization_url && openedFor !== status.flow_id;
}
