# teamcraft help

Task-oriented guide. Every command also exists as an MCP tool ([docs/TOOLS.md](docs/TOOLS.md)); add `--json` for machine-readable output; `teamcraft <command> --help` lists all options.

teamcraft records working time and leave and checks the Arbeitszeitgesetz. It is **not payroll**.

## People

```powershell
teamcraft employee add JM "Joe Mocky" --hours 40 --start 01.04.2023 --kv "IT-KV" --group ST1 --gross "3.400,00"
teamcraft employee add AM "Anna Mockova" --hours "20" --start 15.09.2026
teamcraft employee list
teamcraft employee update AM --hours "25" --gross "1.450,00"
teamcraft employee update AM --inactive
```

Only work-relevant fields exist. There is no field for SV number, address, bank account or health data, on purpose.

## Recording time

```powershell
teamcraft clock-in JM                       # now; --at "2026-11-09 08:00" to correct
teamcraft clock-out JM --break 30
teamcraft time add JM --date 05.11.2026 --start 08:00 --end 16:30 --break 30
teamcraft time add JM --date 05.11.2026 --start 22:00 --end 06:00 --break 30   # night shift into the next day
teamcraft time list --from 01.11.2026 --to 30.11.2026 --employee JM
teamcraft time delete 17                    # only in weeks that are not confirmed
```

Entries cannot overlap and one person can have only one open (clocked-in) entry.

## Confirming a week

```powershell
teamcraft confirm JM 2026-W45
```

After confirmation the week's entries are read-only (enforced twice: in code and by the database). Corrections in a confirmed week need a note to the bookkeeper, not an edit.

## Working-time law (AZG)

```powershell
teamcraft azg --from 01.11.2026 --to 30.11.2026
teamcraft violations
teamcraft violations --all                  # including cleared ones
```

Checked: more than 10 h a day (warning: needs an overtime arrangement), more than 12 h a day, more than 60 h a week, more than 48 h average over 17 weeks, less than 30 min break above 6 h (gaps of 10 min or more count as break), less than 11 h rest between days. Findings are recorded with minutes over or short. Nothing is corrected automatically; when the data is fixed, the next check marks the finding cleared and keeps it in the history.

## Leave

```powershell
teamcraft leave request JM vacation 21.12.2026 --to 31.12.2026
teamcraft leave request JM vacation 13.11.2026 --half
teamcraft leave request AM sick 06.11.2026          # recorded at once; no reason, no note
teamcraft leave list --status requested
teamcraft leave approve 3                           # refused if it overdraws the balance; --force allows it
teamcraft leave reject 4
teamcraft leave balance --year 2026
teamcraft leave adjust JM 2026 "3" --reason "Übertrag aus 2025"
teamcraft holidays 2027
```

Days are counted Monday to Friday minus the 13 Austrian public holidays. Vacation requests stay inside one calendar year. In the start year the entitlement is pro rata by month.

## Pay check against the KV

Import your own KV minimum table (no figures ship with the app):

```text
kv;group;valid_from;monthly_min;weekly_hours
IT-KV;ST1;01.01.2026;2.450,00;38,5
```

```powershell
teamcraft kv import D:\Temp\kv-2026.csv
teamcraft kv check --date 09.11.2026
```

The minimum is pro rata to each person's weekly hours.

## Timesheet and capacity

```powershell
teamcraft timesheet JM 2026-11 --out D:\Temp\jm-2026-11.csv   # AZG section 26 record, opens in Excel
teamcraft capacity 2026-W46
teamcraft capacity 2026-W46 --emit                            # tells taskcraft (shown in taskcraft status)
```

## Troubleshooting

- **"week is confirmed and locked"**: the entry lies in a confirmed week; that is the point of confirming.
- **"overlaps entry 12"**: check `time list` for that day; delete or correct the other entry first.
- **"range contains no working days"**: the leave falls on a weekend or a public holiday.
- **MCP cannot read the KV file or write the timesheet**: start the server with `--allow-path <folder>`.
