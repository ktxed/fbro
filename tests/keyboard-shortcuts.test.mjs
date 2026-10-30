import assert from 'node:assert/strict';
import test from 'node:test';
import { captureStroke, bindingMatches, formatBinding, applyCommandsUpdate, reviewBindingEdit, zoomAction } from '../src/commands/bindings.ts';
const event = (key, extra = {}) => ({ key, code: '', shiftKey: false, ctrlKey: false, altKey: false, metaKey: false, ...extra });
const binding = (key, extra = {}, modes = []) => ({ strokes: [{ key, ...extra }], modes });

test('recorder captures control keys and physical letters without capturing modifiers or composition', () => {
  for (const key of ['Enter', 'Escape', 'Tab', 'F8', ' ', 'x']) assert.equal(captureStroke(event(key)).key, key);
  assert.equal(captureStroke(event('Shift')), null);
  assert.equal(captureStroke(event('Dead')), null);
  assert.equal(captureStroke(event('Process')), null);
  assert.equal(captureStroke(event('å', {code:'KeyA',altKey:true})).key, 'a');
  assert.equal(captureStroke(event('!', {code:'Digit1',shiftKey:true})).key, '1');
});

test('formatting and matching retain modifiers, shifted symbols, physical fallbacks and view contexts', () => {
  assert.equal(formatBinding(binding('F8', {ctrl:true})), 'Ctrl+F8');
  assert.equal(formatBinding(binding(' ')), 'Space');
  assert.equal(bindingMatches(event('ж', {code:'KeyR'}), binding('r')), true);
  assert.equal(bindingMatches(event('+', {shiftKey:true}), binding('+')), true);
  assert.equal(bindingMatches(event('!', {code:'Digit1',shiftKey:true}), binding('1', {shift:true})), true);
  assert.equal(bindingMatches(event('r'), binding('r', {}, ['list']), 'grid'), false);
  assert.equal(bindingMatches(event('r'), binding('r', {}, ['list']), 'list'), true);
  assert.equal(bindingMatches(event('r', {ctrlKey:true}), binding('r')), false);
  assert.equal(bindingMatches(event('r'), {strokes:[],modes:[]}), false);
});

test('versioned full registry update rejects stale delivery and removes old shortcuts', () => {
  const current = {commands:{a:{id:'a',bindings:[binding('a')]}}, commandRevision:4};
  assert.equal(applyCommandsUpdate(current,4,[]), current);
  assert.equal(applyCommandsUpdate(current,3,[]), current);
  const next = applyCommandsUpdate(current,5,[{id:'a',bindings:[]}]);
  assert.equal(next.commandRevision,5);
  assert.deepEqual(next.commands.a.bindings,[]);
  assert.deepEqual(applyCommandsUpdate(next,6,[{id:'b',bindings:[]}]).commands, {b:{id:'b',bindings:[]}});
});

test('physical letter and digit ownership resolves layout ambiguity to one canonical stroke', () => {
  const remapped = event('q', {code:'KeyA'});
  assert.equal(bindingMatches(remapped, binding('a')), true);
  assert.equal(bindingMatches(remapped, binding('q')), false);
  const symbol = event('&', {code:'Digit1'});
  assert.equal(bindingMatches(symbol, binding('1')), true);
  assert.equal(bindingMatches(symbol, binding('&')), false);
  assert.equal(bindingMatches(event('!', {code:'Digit1',shiftKey:true}), binding('!')), false);
});


test('reviewing a stale edit follows the original binding after reordering and refuses removed originals', () => {
  const a = binding('F11'), b = binding('F12');
  assert.deepEqual(reviewBindingEdit([a,b], 1, [b,a]), {bindings:[b,a],index:0});
  assert.equal(reviewBindingEdit([a,b], 1, [a]), null);
  assert.deepEqual(reviewBindingEdit([a], null, [a,b]), {bindings:[a,b],index:null});
  assert.deepEqual(reviewBindingEdit([binding('q')], 0, [{strokes:[{key:'Q',shift:false}],modes:['grid','list']}]), {bindings:[{strokes:[{key:'Q',shift:false}],modes:['grid','list']}],index:0});
});

test('dispatch agrees with backend conflict fixtures across keys, modifiers and contexts', async () => {
  const { readFileSync } = await import('node:fs');
  const fixtures = JSON.parse(readFileSync(new URL('./fixtures/keyboard-binding-semantics.json', import.meta.url), 'utf8'));
  for (const fixture of fixtures) {
    const e = event(fixture.event.key, fixture.event);
    const overlap = ['list', 'grid'].some(mode => bindingMatches(e, fixture.left, mode) && bindingMatches(e, fixture.right, mode));
    assert.equal(overlap, fixture.overlap, JSON.stringify(fixture));
  }
});


test('reserved zoom uses the same canonical key as recording and command matching', () => {
  assert.equal(zoomAction(event('+', {code:'Digit1',ctrlKey:true,shiftKey:true}), false), null);
  assert.equal(zoomAction(event('+', {code:'Equal',ctrlKey:true,shiftKey:true}), false), 1);
  assert.equal(zoomAction(event('-', {ctrlKey:true}), false), -1);
  assert.equal(zoomAction(event(')', {code:'Digit0',ctrlKey:true,shiftKey:true}), false), 0);
  assert.equal(zoomAction(event('-', {metaKey:true}), true), -1);
  assert.equal(zoomAction(event('-', {ctrlKey:true}), true), null);
});

test('history brackets keep their logical symbol when WebKit reports a physical digit', () => {
  for (const [symbol, digit] of [['[', '8'], [']', '9']]) {
    const e = event(symbol, {code:`Digit${digit}`});
    assert.equal(bindingMatches(e, binding(symbol)), true);
    assert.equal(bindingMatches(e, binding(digit)), false);
    assert.equal(captureStroke(e).key, symbol);
    assert.equal(bindingMatches(event(digit, {code:`Digit${digit}`}), binding(digit)), true);
  }
});
