## Pre-requisites

On Windows, Microsoft Visual C++ 14.0 or greater is required.
Get it with "Microsoft C++ Build Tools": https://visualstudio.microsoft.com/visual-cpp-build-tools/

Once installed, "Modify" it, go to Individual Components, select only:

- Windows SDK (latest)
- C++ x64/x86 build tools (latest)

## Setup

Install Python v3.11. On Windows, I used Scoop in a Powershell terminal.

```PowerShell
scoop install python@3.11.4
```

In IntelliJ, go to Project Structure -> SDKs, add a new Python (virtualenv). In Project, select the SDK.

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
black src/ tests/
```

## Type-check code

```PowerShell
mypy src/
mypy tests/
```

## Run tests

```Powershell
python -m unittest discover --start-directory tests/
```
