import { describe, expect, it, vi } from 'vitest';
import { readdirSync, readFileSync } from 'node:fs';
import { dirname, extname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { en, ru } from './dictionaries';
import { createLocaleManager, detectLocale, LOCALE_STORAGE_KEY, type LocaleStorage } from './locale';

class MemoryStorage implements LocaleStorage {
  private values = new Map<string, string>();
  getItem(key: string): string | null { return this.values.get(key) ?? null; }
  setItem(key: string, value: string): void { this.values.set(key, value); }
}

function leafKeys(value: unknown, prefix = ''): string[] {
  if (!value || typeof value !== 'object') return [prefix];
  return Object.entries(value).flatMap(([key, child]) => leafKeys(child, prefix ? `${prefix}.${key}` : key));
}

function hasTranslationPath(dictionary: unknown, path: string): boolean {
  let current: unknown = dictionary;
  for (const segment of path.split('.')) {
    if (!current || typeof current !== 'object' || !(segment in current)) return false;
    current = (current as Record<string, unknown>)[segment];
  }
  return typeof current === 'string';
}

function sourceFiles(root: string): string[] {
  const files: string[] = [];
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    const path = join(root, entry.name);
    if (entry.isDirectory()) {
      if (!/^(__tests__|fixtures)$/i.test(entry.name)) files.push(...sourceFiles(path));
      continue;
    }
    if (!entry.isFile() || !/\.tsx?$/.test(extname(entry.name))) continue;
    if (/\.(?:test|spec)\.tsx?$/.test(entry.name)) continue;
    files.push(path);
  }
  return files;
}

function literalTranslationCalls(root: string): Map<string, string[]> {
  const calls = new Map<string, string[]>();
  const literalCall = /\bt\s*\(\s*(["'])([^"']+)\1/g;
  for (const path of sourceFiles(root)) {
    const text = readFileSync(path, 'utf8');
    for (const match of text.matchAll(literalCall)) {
      const key = match[2];
      const locations = calls.get(key) ?? [];
      locations.push(relative(root, path));
      calls.set(key, locations);
    }
  }
  return calls;
}

describe('Qlisa i18n core', () => {
  it('keeps Russian and English dictionary leaf keys in parity', () => {
    expect(leafKeys(ru).sort()).toEqual(leafKeys(en).sort());
  });

  it('uses only translation paths present in both locale dictionaries', () => {
    const srcRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
    const missing = [...literalTranslationCalls(srcRoot).entries()]
      .filter(([key]) => !hasTranslationPath(en, key) || !hasTranslationPath(ru, key))
      .map(([key, files]) => `${key} (${files.join(', ')})`)
      .sort();
    expect(missing).toEqual([]);
  });

  it('interpolates safe values and preserves missing placeholders', () => {
    const manager = createLocaleManager({ language: 'en-US', storage: new MemoryStorage(), listenToStorage: false });
    expect(manager.t('app.workspaceTitle', { name: 'Show 1' })).toBe('Show 1 — Qlisa');
    expect(manager.t('errors.noAudioTrack', { path: 'clip.wav' })).toBe('No audio track in file: clip.wav');
    expect(manager.t('errors.noAudioTrack')).toBe('No audio track in file: {path}');
    manager.dispose();
  });

  it('uses Russian for Russian language and English for English language', () => {
    const storage = new MemoryStorage();
    const ruManager = createLocaleManager({ language: 'ru-RU', storage, listenToStorage: false });
    const enManager = createLocaleManager({ language: 'en-US', storage: new MemoryStorage(), listenToStorage: false });
    expect(ruManager.t('common.cancel')).toBe('Отмена');
    expect(enManager.t('common.cancel')).toBe('Cancel');
    ruManager.dispose();
    enManager.dispose();
  });

  it('keeps cue and media editor labels consistent in Russian', () => {
    const russianText = JSON.stringify(ru);
    expect(russianText).not.toMatch(/кью|корзин|вживую|срезы/i);
    expect(ru.menus.livePanel).toBe('Timeline');
    expect(ru.menus.slicePanel).toBe('Slice');
    expect(ru.editorUi.liveTab).toBe('Timeline');
    expect(ru.editorUi.sliceTab).toBe('Slice');
    expect(ru.editorUi.emptyCueTitle).toContain('cue');
    expect(ru.editorUi.emptyCueMessage).toContain('cue');
    expect(ru.sweepUi.switchCart).toContain('режим карточек');
  });

  it('keeps Russian command cue copy concrete and English copy unchanged', () => {
    expect(ru.actions.arm).toBe('Разрешить запуск');
    expect(ru.actions.disarm).toBe('Запретить запуск');
    expect(ru.cueTypes.arm).toBe('Разрешить запуск');
    expect(ru.cueTypes.disarm).toBe('Запретить запуск');
    expect(ru.inspectorCueUi.commandTarget).toBe('Выбранный cue');
    expect(ru.inspectorCueUi.commandTargets).toBe('Выбранные cue');
    expect(ru.inspectorCueUi.devampTargets).toBe('Выбранные cue');
    expect(ru.inspectorCueUi.fadeTargets).toBe('Выбранные cue');
    expect(ru.inspectorCueUi.stopTargets).toBe('Какие cue остановить');
    expect(ru.sweepUi.targets).toBe('Выбранные cue');
    expect(ru.toolbar.addDevamp).toContain('закончить текущий повтор и продолжить воспроизведение дальше');
    expect(ru.toolbar.commandHintStart).toBe('Запустить выбранный cue');
    expect(ru.toolbar.commandHintPause).toBe('Поставить выбранный cue на паузу');
    expect(ru.toolbar.commandHintResume).toBe('Продолжить выбранный cue после паузы');
    expect(ru.toolbar.commandHintLoad).toBe('Заранее загрузить выбранный cue и оставить его на паузе');
    expect(ru.toolbar.commandHintReset).toBe('Остановить выбранный cue и вернуть его в начало');
    expect(ru.toolbar.commandHintGoto).toBe('Переместить указатель GO к выбранному cue');
    expect(ru.toolbar.commandHintArm).toBe('Разрешить запуск выбранного cue');
    expect(ru.toolbar.commandHintDisarm).toBe('Запретить запуск выбранного cue');
    expect([
      ru.toolbar.commandHintStart,
      ru.toolbar.commandHintPause,
      ru.toolbar.commandHintResume,
      ru.toolbar.commandHintLoad,
      ru.toolbar.commandHintReset,
      ru.toolbar.commandHintGoto,
      ru.toolbar.commandHintArm,
      ru.toolbar.commandHintDisarm,
    ].join(' ')).not.toMatch(/цель|цел[иеяй]/i);

    expect(en.actions.arm).toBe('Arm');
    expect(en.actions.disarm).toBe('Disarm');
    expect(en.inspectorCueUi.commandTarget).toBe('Target');
    expect(en.inspectorCueUi.commandTargets).toBe('Targets');
    expect(en.inspectorCueUi.devampTargets).toBe('Targets');
    expect(en.inspectorCueUi.fadeTargets).toBe('Targets');
    expect(en.inspectorCueUi.stopTargets).toBe('Targets');
    expect(en.sweepUi.targets).toBe('Targets');
    expect(en.toolbar.addDevamp).toBe('Add Devamp Cue (release a vamping slice loop) · Drag to insert at position');
    expect(en.toolbar.commandHintStart).toBe('Trigger the targets');
    expect(en.toolbar.commandHintPause).toBe('Pause the targets');
    expect(en.toolbar.commandHintResume).toBe('Resume the targets');
    expect(en.toolbar.commandHintLoad).toBe('Bring them up paused');
    expect(en.toolbar.commandHintReset).toBe('Return them to standby');
    expect(en.toolbar.commandHintGoto).toBe('Move the Playhead there');
    expect(en.toolbar.commandHintArm).toBe('Enable the targets');
    expect(en.toolbar.commandHintDisarm).toBe('Disable the targets');
  });

  it('provides bilingual multi-cue Inspector labels without translating cue timing terms', () => {
    expect(ru.multiCueInspector.title).toBe('{count} cue выбрано');
    expect(ru.multiCueInspector.mixed).toBe('Разные значения');
    expect(ru.multiCueInspector.apply).toBe('Применить к {count} cue');
    expect(ru.multiCueInspector.discard).toBe('Отменить изменения');
    expect(ru.multiCueInspector.preWait).toBe('Pre-Wait');
    expect(ru.multiCueInspector.postWait).toBe('Post-Wait');
    expect(ru.multiCueInspector.stopCueToEditFades).toBe('Остановите выбранные cue, чтобы изменить фейды.');
    expect(en.multiCueInspector.title).toBe('{count} cues selected');
    expect(en.multiCueInspector.mixed).toBe('Different values');
    expect(en.multiCueInspector.apply).toBe('Apply to {count} cues');
    expect(en.multiCueInspector.stopCueToEditFades).toBe('Stop the selected cues to edit fades.');
  });

  it('provides bilingual Active Cues panel labels', () => {
    expect(en.menus.activeCues).toBe('Active Cues');
    expect(en.activeCues.empty).toBe('No active cues');
    expect(en.activeCues.pauseUnavailable).toBe('Pause is not supported for this cue type');
    expect(ru.menus.activeCues).toBe('Активные Cue');
    expect(ru.activeCues.empty).toBe('Нет активных cue');
    expect(ru.activeCues.pauseUnavailable).toBe('Для этого типа cue пауза не поддерживается');
  });

  it('provides bilingual single-cue Inspector save errors', () => {
    expect(en.inspector.saveError).toBe('Could not save cue changes: {error}');
    expect(en.inspector.stopCueBeforeFadeEdit).toBe('Stop the cue before changing fades.');
    expect(ru.inspector.saveError).toBe('Не удалось сохранить изменения cue: {error}');
    expect(ru.inspector.stopCueBeforeFadeEdit).toBe('Остановите cue перед изменением фейда.');
  });

  it('falls back to English and warns once for a missing locale key', () => {
    const warn = vi.fn();
    const manager = createLocaleManager({ language: 'ru', storage: new MemoryStorage(), warn, listenToStorage: false });
    expect(manager.t('not.in.dictionary')).toBe('not.in.dictionary');
    expect(manager.t('not.in.dictionary')).toBe('not.in.dictionary');
    expect(warn).toHaveBeenCalledTimes(1);
    expect(warn.mock.calls[0][0]).toContain('not.in.dictionary');
    manager.dispose();
  });

  it('detects saved override before language and defaults to Russian when unavailable', () => {
    const storage = new MemoryStorage();
    expect(detectLocale('en-US', storage)).toBe('en');
    storage.setItem(LOCALE_STORAGE_KEY, 'ru');
    expect(detectLocale('en-US', storage)).toBe('ru');
    expect(detectLocale(undefined, new MemoryStorage())).toBe('ru');
    expect(detectLocale('de-DE', new MemoryStorage())).toBe('ru');
  });

  it('persists locale changes and notifies subscribers', () => {
    const storage = new MemoryStorage();
    const manager = createLocaleManager({ language: 'ru', storage, listenToStorage: false });
    const listener = vi.fn();
    const unsubscribe = manager.subscribe(listener);
    manager.setLocale('en');
    expect(manager.getLocale()).toBe('en');
    expect(storage.getItem(LOCALE_STORAGE_KEY)).toBe('en');
    expect(listener).toHaveBeenCalledWith('en');
    unsubscribe();
    manager.setLocale('ru');
    expect(listener).toHaveBeenCalledTimes(1);
    manager.dispose();
  });
});
