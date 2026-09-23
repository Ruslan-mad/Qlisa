import { useSyncExternalStore } from "react";
import { getLocale, setLocale, subscribeLocale, t } from "./locale";

/** React binding for the shared locale manager. */
export function useLocale() {
  const locale = useSyncExternalStore(subscribeLocale, getLocale, getLocale);
  return { locale, t, setLocale };
}
