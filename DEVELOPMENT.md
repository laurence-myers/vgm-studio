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

## Run

```PowerShell
.\venv\Scripts\python.exe -m src.drotrimmer.drotrim 
```

## Build .exe

```PowerShell
cd src
..\venv\Scripts\python.exe setup.py
```
