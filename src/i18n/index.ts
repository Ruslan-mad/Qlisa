export { en, ru, type TranslationDictionary } from './dictionaries';
export {
  LOCALE_STORAGE_KEY,
  createLocaleManager,
  detectLocale,
  getLocale,
  localeManager,
  setLocale,
  subscribeLocale,
  t,
  type Locale,
  type LocaleListener,
  type LocaleManager,
  type LocaleManagerOptions,
  type LocaleStorage,
  type TranslationKey,
  type TranslationVars,
} from './locale';
export { useLocale } from './useLocale';
