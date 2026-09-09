import type { AgentProvider, AgentView } from "./types.js";

export interface DesktopDaemonRuntime {
  allowDevelopmentMock: boolean;
  databaseFilename: string;
  endpoint: string;
  listen: string;
  startupTimeoutMs: number;
}

export function desktopDaemonRuntime(isPackaged: boolean): DesktopDaemonRuntime {
  const port = isPackaged ? 7314 : 7315;
  return {
    allowDevelopmentMock: !isPackaged,
    databaseFilename: isPackaged ? "ait.sqlite3" : "ait-development.sqlite3",
    endpoint: `http://127.0.0.1:${port}`,
    listen: `127.0.0.1:${port}`,
    startupTimeoutMs: isPackaged ? 15_000 : 120_000,
  };
}

function isDevelopmentMockProvider(provider: AgentProvider): boolean {
  return provider.id === "builtin-mock" || provider.kind === "mock";
}

export function desktopProviderCatalog(
  providers: AgentProvider[],
  agents: AgentView[],
  allowDevelopmentMock: boolean,
): { providers: AgentProvider[]; agents: AgentView[] } {
  if (allowDevelopmentMock) return { providers, agents };
  const hiddenProviderIds = new Set(
    providers.filter(isDevelopmentMockProvider).map((provider) => provider.id),
  );
  hiddenProviderIds.add("builtin-mock");
  return {
    providers: providers.filter((provider) => !isDevelopmentMockProvider(provider)),
    agents: agents.filter((agent) => !hiddenProviderIds.has(agent.config.provider_id)),
  };
}
