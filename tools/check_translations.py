"""Audit application-owned messages and the Polish/English translation catalog."""
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
catalog = json.loads((ROOT / 'src/translations.json').read_text(encoding='utf-8'))
known = {entry[language] for entry in catalog for language in ('pl', 'en')}


def normalized(text):
    indices = iter(range(100))
    return re.sub(r'\{[^{}]*\}', lambda _: '{' + str(next(indices)) + '}', text)


for entry in catalog:
    assert entry['pl'].strip() and entry['en'].strip(), entry
    for language in ('pl', 'en'):
        indices = re.findall(r'\{(\d+)\}', entry[language])
        assert indices == [str(i) for i in range(len(indices))], entry
    assert re.findall(r'\{\d+\}', entry['pl']) == re.findall(r'\{\d+\}', entry['en']), entry

# Proper names, language autonyms, paths, file formats, UI IDs and numeric units
# are not natural-language messages. CLI syntax uses stable command identifiers.
technical = {
    'png-export', 'controls', 'language', 'Polski', 'English', 'Terrain Builder',
    'TV4P', 'tv4p', 'output.tv4p', 'PNG', 'PNG:', 'png', 'export', 'scope',
    'roads', 'road_kind', 'E', 'N', '100 m', '../app_icon.png',
    'Terrain Builder Road Merger', '  {} 0x{:02X} {}',
    'A…', 'B…', 'MLOD', 'px', 'm',
    'TERRAIN BUILDER / DAYZ', 'DAYZ ROAD TOOLS', 'Merge', 'Road Builder',
    'toolsets', 'shell-language', 'DayZ Road Tools {}', 'DayZ Road Tools',
    'ui-screenshot', 'EFRAME_SCREENSHOT_TO', 'DAYZ_SCREENSHOT_TOOLSET', 'builder', 'merge',
}
missing = []
for filename in ('gui.rs', 'shell.rs', 'main.rs', 'tv4p.rs', 'render.rs', 'geometry.rs'):
    source = (ROOT / 'src' / filename).read_text(encoding='utf-8').split('#[cfg(test)]')[0]
    for match in re.finditer(r'"(?:\\.|[^"\\])*"', source):
        try:
            text = json.loads(match.group())
        except ValueError:
            continue  # Rust byte strings are not application messages.
        plain = re.sub(r'\{[^{}]*\}', '', text)
        if not any(char.isalpha() for char in plain) or text in technical:
            continue
        if filename not in ('gui.rs', 'shell.rs'):
            prefix = source[max(0, match.start() - 90):match.start()]
            if not re.search(r'(bail!|anyhow!|context|format!|println!|write!)\s*\([^";]*$', prefix):
                continue
            if text.startswith('GUI:'):
                continue
        if normalized(text) not in known:
            line = source.count('\n', 0, match.start()) + 1
            missing.append((filename, line, text))
assert not missing, missing
print(f'{len(catalog)} complete PL/EN pairs; matching placeholders; no untranslated application messages.')

builder = ROOT / 'crates' / 'road-builder' / 'src'
catalog = json.loads((builder / 'translations.json').read_text(encoding='utf-8'))
known = {entry[language] for entry in catalog for language in ('pl', 'en')}
for entry in catalog:
    assert entry['pl'].strip() and entry['en'].strip(), entry
    for language in ('pl', 'en'):
        indices = re.findall(r'\{(\d+)\}', entry[language])
        assert indices == [str(i) for i in range(len(indices))], entry
    assert re.findall(r'\{\d+\}', entry['pl']) == re.findall(r'\{\d+\}', entry['en']), entry

missing = []
for path in builder.glob('*.rs'):
    if path.name == 'i18n.rs':
        continue
    source = path.read_text(encoding='utf-8').split('#[cfg(test)]')[0]
    for match in re.finditer(r'"(?:\\.|[^"\\])*"', source):
        try:
            text = json.loads(match.group())
        except ValueError:
            continue
        # All Polish messages, plus the English diagnostics inherited from Merge.
        prefix = source[max(0, match.start() - 90):match.start()]
        message = re.search(r'[ąćęłńóśźżĄĆĘŁŃÓŚŹŻ]', text) or (
            re.search(r'(bail!|anyhow!|ensure!|context)\s*\([^";]*$', prefix)
            and re.search(r'[A-Za-z]', text)
        )
        if message and text not in {'MLOD', 'BM'} and normalized(text) not in known:
            line = source.count('\n', 0, match.start()) + 1
            missing.append((path.name, line, text))
assert not missing, missing
assert len({entry['pl'] for entry in catalog}) == len(catalog), 'Duplicate Builder translation keys'
print(f'Road Builder: {len(catalog)} complete PL/EN pairs; matching placeholders; all diagnostics covered.')
