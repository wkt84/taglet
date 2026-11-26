import assert from 'node:assert/strict'
import test from 'node:test'
import { startupSteps } from '../src/data/startupSteps.ts'

const marks = [
  { name: 'process', at_ms: 1000 },
  { name: 'run', at_ms: 1100 },
  { name: 'built', at_ms: 1130 },
  { name: 'window', at_ms: 1400 },
  { name: 'ready', at_ms: 1401 },
  { name: 'page_load', at_ms: 1420 },
]

test('startup is split into steps ending at each moment', () => {
  assert.deepEqual(startupSteps(marks, 1450, 1550), {
    totalMs: 550,
    steps: [
      { name: 'process', ms: 100 },
      { name: 'init', ms: 30 },
      { name: 'window', ms: 270 },
      { name: 'load', ms: 50 },
      { name: 'page', ms: 100 },
    ],
  })
})

test('without the process creation (not Windows), startup counts from run', () => {
  assert.deepEqual(startupSteps(marks.slice(1), 1450, 1550), {
    totalMs: 450,
    steps: [
      { name: 'init', ms: 30 },
      { name: 'window', ms: 270 },
      { name: 'load', ms: 50 },
      { name: 'page', ms: 100 },
    ],
  })
})

test('a missing moment merges its step into the next', () => {
  const withoutWindow = marks.filter((mark) => mark.name !== 'window')
  assert.deepEqual(startupSteps(withoutWindow, 1450, 1550).steps.map((step) => [step.name, step.ms]), [
    ['process', 100],
    ['init', 30],
    ['load', 320],
    ['page', 100],
  ])
})

test('when the page starts loading before the webview is reported created, load comes first', () => {
  assert.deepEqual(startupSteps(marks, 1300, 1550), {
    totalMs: 550,
    steps: [
      { name: 'process', ms: 100 },
      { name: 'init', ms: 30 },
      { name: 'load', ms: 170 },
      { name: 'window', ms: 100 },
      { name: 'page', ms: 150 },
    ],
  })
})
