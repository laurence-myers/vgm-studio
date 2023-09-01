## Setup

Install Python v3.11. On Windows, I used Scoop in a Powershell terminal.

```PowerShell
scoop install python@3.11.4
```

Install normal dependencies.

```PowerShell
py -m pip install -r requirements.txt
```

Install dev dependencies.

```PowerShell
py -m pip install -r requirements_dev.txt
```

Set up Black for auto-formatting: [here's how to set it up in IntelliJ or PyCharm.](https://www.jetbrains.com/help/pycharm/2023.2/reformat-and-rearrange-code.html#format-python-code-with-black)

Set up Git to ignore bulk change commits (like auto-formatting) when running "blame".

```PowerShell
git config blame.ignoreRevsFile .git-blame-ignore-revs
```

## Run

```PowerShell
.\venv\Scripts\python.exe -m src.drotrimmer.drotrim 
```

## Build .exe

```PowerShell
cd src
..\venv\Scripts\python.exe setup.py
```

## Format code

```PowerShell
black src/
```

## Type-check code

```PowerShell
mypy src/
```

## Run tests

```Powershell
python -m unittest discover --start-directory tests/
```
