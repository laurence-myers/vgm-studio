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

## Run

```PowerShell
.\venv\Scripts\python.exe -m src.drotrimmer.drotrim 
```

## Build .exe

```PowerShell
cd src
..\venv\Scripts\python.exe setup.py
```
