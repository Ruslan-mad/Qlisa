import { en, ru, type TranslationDictionary } from './dictionaries';

export type Locale = 'ru' | 'en';
export const LOCALE_STORAGE_KEY = 'qlisa_locale';

export interface LocaleStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

export interface LocaleManagerOptions {
  storage?: LocaleStorage | null;
  language?: string | null;
  warn?: (message: string) => void;
  listenToStorage?: boolean;
}

const isLocale = (value: unknown): value is Locale => value === 'ru' || value === 'en';

function browserStorage(): LocaleStorage | null {
  try {
    return typeof window !== 'undefined' && window.localStorage ? window.localStorage : null;
  } catch {
    return null;
  }
}

function browserLanguage(): string | null {
  return typeof navigator !== 'undefined' ? navigator.language : null;
}

/** Detects the persisted choice first, then the browser language. Russian is the safe product default. */
export function detectLocale(
  language: string | null | undefined = browserLanguage(),
  storage: LocaleStorage | null | undefined = browserStorage(),
): Locale {
  try {
    const saved = storage?.getItem(LOCALE_STORAGE_KEY);
    if (isLocale(saved)) return saved;
  } catch {
    // Storage can be unavailable in private/restricted browser contexts.
  }
  if (typeof language === 'string' && language.toLowerCase().startsWith('en')) return 'en';
  return 'ru';
}

type LeafPath<T, Prefix extends string = ''> = {
  [K in keyof T & string]: T[K] extends string
    ? `${Prefix}${K}`
    : T[K] extends Record<string, unknown>
      ? LeafPath<T[K], `${Prefix}${K}.`>
      : never;
}[keyof T & string];

export type TranslationKey = LeafPath<TranslationDictionary>;
export type TranslationVars = Record<string, string | number | boolean | null | undefined>;
export type LocaleListener = (locale: Locale) => void;

function lookup(dictionary: TranslationDictionary, key: string): string | undefined {
  const value = key.split('.').reduce<unknown>((current, segment) => {
    if (!current || typeof current !== 'object') return undefined;
    return (current as Record<string, unknown>)[segment];
  }, dictionary);
  return typeof value === 'string' ? value : undefined;
}

function interpolate(value: string, vars?: TranslationVars): string {
  if (!vars) return value;
  return value.replace(/\{([\w.-]+)\}/g, (token, name: string) => {
    if (!Object.prototype.hasOwnProperty.call(vars, name)) return token;
    const replacement = vars[name];
    return replacement === null || replacement === undefined ? '' : String(replacement);
  });
}

export interface LocaleManager {
  getLocale(): Locale;
  setLocale(locale: Locale): void;
  subscribe(listener: LocaleListener): () => void;
  t(key: TranslationKey | string, vars?: TranslationVars): string;
  dispose(): void;
}

export function createLocaleManager(options: LocaleManagerOptions = {}): LocaleManager {
  const storage = options.storage === undefined ? browserStorage() : options.storage;
  const warn = options.warn ?? ((message: string) => console.warn(message));
  const listeners = new Set<LocaleListener>();
  const warned = new Set<string>();
  let locale = detectLocale(options.language === undefined ? browserLanguage() : options.language, storage);

  const onStorage = (event: StorageEvent): void => {
    if (event.key !== LOCALE_STORAGE_KEY || !isLocale(event.newValue) || event.newValue === locale) return;
    locale = event.newValue;
    listeners.forEach((listener) => listener(locale));
  };
  const shouldListen = options.listenToStorage !== false;
  if (shouldListen && typeof window !== 'undefined') window.addEventListener('storage', onStorage);

  return {
    getLocale: () => locale,
    setLocale: (next: Locale) => {
      if (!isLocale(next) || next === locale) return;
      locale = next;
      try { storage?.setItem(LOCALE_STORAGE_KEY, next); } catch { /* persistence is best effort */ }
      listeners.forEach((listener) => listener(locale));
    },
    subscribe: (listener: LocaleListener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    t: (key: TranslationKey | string, vars?: TranslationVars) => {
      const current = lookup(locale === 'ru' ? ru : en, key);
      if (current !== undefined) return interpolate(current, vars);
      const fallback = lookup(en, key);
      const warningKey = `${locale}:${key}`;
      if (!warned.has(warningKey)) {
        warned.add(warningKey);
        warn(`[i18n] Missing translation key "${key}" for locale "${locale}"${fallback === undefined ? '' : '; using English fallback'}.`);
      }
      return interpolate(fallback ?? key, vars);
    },
    dispose: () => {
      if (shouldListen && typeof window !== 'undefined') window.removeEventListener('storage', onStorage);
      listeners.clear();
    },
  };
}

export const localeManager = createLocaleManager();
export const getLocale = (): Locale => localeManager.getLocale();
export const setLocale = (locale: Locale): void => localeManager.setLocale(locale);
export const subscribeLocale = (listener: LocaleListener): (() => void) => localeManager.subscribe(listener);
export const t = (key: TranslationKey | string, vars?: TranslationVars): string => localeManager.t(key, vars);

