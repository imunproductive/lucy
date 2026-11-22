import {test} from 'node:test';
import assert from 'node:assert/strict';
import {changes, draftOf, rebase} from './draft.ts';
import {duration, frameForLevel, frameName, initials, levelParts, previewUrl, totalLevel} from './format.ts';
import {legacyPath, matchPath, parseRoute} from './routes.ts';
import {boxColour, boxPicture} from './boxes.ts';

test('routes parse every page, a match id and unknown paths', () => {
  assert.deepEqual(parseRoute('/'), {name: 'profile'});
  assert.deepEqual(parseRoute('/collection/'), {name: 'collection'});
  assert.deepEqual(parseRoute('/matches'), {name: 'matches'});
  assert.deepEqual(parseRoute(matchPath('0123abcd')), {name: 'match', id: '0123abcd'});
  assert.deepEqual(parseRoute('/matches/a/b'), {name: 'not-found'});
  assert.deepEqual(parseRoute('/plate'), {name: 'not-found'});
  assert.equal(legacyPath('#shop'), '/collection');
  assert.equal(legacyPath('#plate'), null);
});

test('a refresh keeps typed values and takes the untouched ones', () => {
  // An input's value can arrive as text; the comparison is by number.
  type Row = {credits: number | string; player_name: string; unlock_all: boolean};
  const fields = ['credits', 'player_name', 'unlock_all'] as const;
  const before: Row = {credits: 10, player_name: 'Alpha', unlock_all: false};
  const after: Row = {credits: 11, player_name: 'Alpha2', unlock_all: true};
  const draft: Row = draftOf(before, fields);
  draft.credits = '25';
  assert.deepEqual(changes(draft, before, fields), {credits: '25'});
  rebase(draft, before, after, fields);
  assert.deepEqual(draft, {credits: '25', player_name: 'Alpha2', unlock_all: true});
  // Typing the server's value back is no change.
  draft.credits = '11';
  assert.deepEqual(changes(draft, after, fields), {});
});

test('levels split into tier, stars and the shown level, and back', () => {
  assert.deepEqual(levelParts(1), {tier: 0, stars: 0, shown: 1});
  assert.deepEqual(levelParts(1441), {tier: 2, stars: 2, shown: 41});
  assert.deepEqual(levelParts(99999), {tier: 4, stars: 5, shown: 100});
  assert.equal(totalLevel({tier: 2, stars: 2, shown: 41}), 1441);
  assert.equal(totalLevel({tier: 0, stars: 0, shown: 500}), 100);
  assert.equal(frameName(1441), 'Gold ★★');
  assert.equal(frameName(1), 'Bronze');
  const frames = [
    {level: 101, guid: '0xB'},
    {level: 1, guid: '0xA'},
    {level: 201, guid: '0xC'},
  ];
  assert.equal(frameForLevel(frames, 150)?.guid, '0xB');
  assert.equal(frameForLevel(frames, 0), null);
});

test('small formatters', () => {
  assert.equal(duration(42), '42 s');
  assert.equal(duration(190), '3 min 10 s');
  assert.equal(duration(7500), '2 h 5 min');
  assert.equal(previewUrl('0x02500000000011ab'), '/assets/previews/02500000000011AB.webp');
  assert.equal(initials('Soldier: 76'), 'S7');
  assert.equal(initials('!!'), '?');
  assert.equal(boxPicture({id: 1, name: 'x', label: 'Halloween Terror'}), '/assets/boxes/halloween.png');
  assert.equal(boxColour({id: 9, name: 'x', label: 'Unknown'}), '#5b82a8');
});
