#!/usr/bin/env python3
import json, sys
b = json.load(open('/tmp/test-batch.json'))
print(len(b['tasks']), 'tasks')
t = b['tasks'][0]
print('id:', t['id'])
print('scope:', t['scope'])
print('touch:', t['touch'])
print('=== accept ===')
print(t['accept'])
print('=== prose (first 300) ===')
print(t['acceptance_prose'][:300])
