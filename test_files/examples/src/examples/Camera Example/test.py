from pathlib import Path

print('Workspace files:')
for path in sorted(Path('.').glob('**/*')):
    if path.is_file() and '.mercurio' not in path.parts:
        print(path)
