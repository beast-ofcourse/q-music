import { createContext, useContext, type ReactNode } from "react";

type SettingsLauncher = (extensionId: string) => void;

const SettingsLauncherContext = createContext<SettingsLauncher | null>(null);

interface SettingsLauncherProviderProps {
  launch: SettingsLauncher;
  children: ReactNode;
}

/**
 * The Settings trigger lives in the installed footer both card types share. A context
 * keeps the handler out of two card components that differ only in what they know about
 * a package, and it means a package installed from a file gets the trigger too.
 */
export function SettingsLauncherProvider({ launch, children }: SettingsLauncherProviderProps) {
  return (
    <SettingsLauncherContext.Provider value={launch}>
      {children}
    </SettingsLauncherContext.Provider>
  );
}

export function useSettingsLauncher(): SettingsLauncher | null {
  return useContext(SettingsLauncherContext);
}
