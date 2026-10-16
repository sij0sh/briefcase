#!/usr/bin/env python3
"""Local provider bridge: workflow request on stdin, model reply on stdout.

Reads a Briefcase lineage/domain inference request, calls the configured
OpenRouter chat model with temperature 0, and prints the reply JSON the
workflow validator expects. Provider output stays proposal-only; the
workflow accepts or rejects it deterministically.

Usage: openrouter-provider.py lineage|domain [--model MODEL]
"""
import json
import os
import re
import subprocess
import sys

API = 'https://openrouter.ai/api/v1/chat/completions'


def extract_json(text):
    fence = re.search(r'```(?:json)?\s*(\{.*?\})\s*```', text, re.DOTALL)
    if fence:
        return fence.group(1)
    start = text.find('{')
    end = text.rfind('}')
    if start >= 0 and end > start:
        return text[start:end + 1]
    return None


def main():
    kind = sys.argv[1] if len(sys.argv) > 1 else 'lineage'
    model = 'openai/gpt-4o-mini'
    if '--model' in sys.argv:
        model = sys.argv[sys.argv.index('--model') + 1]
    request = json.load(sys.stdin)
    system = ('You are a precise code-analysis assistant. Return exactly one JSON object, no markdown, '
              'no commentary. Use only ids, predicates, and assertion references given in the request. '
              'Never invent identifiers.')
    user = json.dumps(request)
    body = json.dumps({'model': model, 'temperature': 0,
                       'messages': [{'role': 'system', 'content': system},
                                    {'role': 'user', 'content': user}],
                       'max_tokens': 2000})
    key = os.environ.get('OPENROUTER_API_KEY', '')
    if not key:
        print(json.dumps({'proposals': [], 'abstained': True,
                          'limitations': ['provider misconfigured: no OPENROUTER_API_KEY']}))
        return
    result = subprocess.run(['curl', '-s', '-m', '120', API,
                             '-H', f'Authorization: Bearer {key}',
                             '-H', 'Content-Type: application/json',
                             '-H', 'HTTP-Referer: https://briefcase.local/eval',
                             '-d', body],
                            capture_output=True, text=True, timeout=150)
    try:
        response = json.loads(result.stdout)
    except json.JSONDecodeError:
        print(json.dumps({'proposals': [], 'abstained': True,
                          'limitations': [f'model transport failure: {result.stderr[:200] or result.stdout[:200]}']}))
        return
    if 'error' in response:
        print(json.dumps({'proposals': [], 'abstained': True,
                          'limitations': [f"model error: {str(response['error'])[:300]}"]}))
        return
    content = response['choices'][0]['message']['content'] or ''
    usage = response.get('usage', {})
    print(json.dumps({'provider_usage': usage, 'model': response.get('model', model)}),
          file=sys.stderr)
    raw = extract_json(content)
    if raw is None:
        print(json.dumps({'proposals': [], 'abstained': True,
                          'limitations': ['model returned no JSON object; abstained rather than inventing output']}))
        return
    try:
        reply = json.loads(raw)
    except json.JSONDecodeError:
        print(json.dumps({'proposals': [], 'abstained': True,
                          'limitations': ['model returned malformed JSON; abstained rather than inventing output']}))
        return
    # Normalize container types to the workflow reply shape without adding
    # content. The workflow still validates every id, predicate, and support.
    proposals = reply.get('proposals', [])
    if isinstance(proposals, dict):
        proposals = [proposals]
    if not isinstance(proposals, list):
        proposals = []
    for proposal in proposals:
        if isinstance(proposal, dict):
            if isinstance(proposal.get('limitations'), str):
                proposal['limitations'] = [proposal['limitations']]
            proposal.setdefault('limitations', [])
    limitations = reply.get('limitations', [])
    if isinstance(limitations, str):
        limitations = [limitations]
    if not isinstance(limitations, list):
        limitations = []
    reply['proposals'] = proposals
    reply['limitations'] = limitations
    reply['abstained'] = not proposals
    print(json.dumps(reply))


if __name__ == '__main__':
    main()
