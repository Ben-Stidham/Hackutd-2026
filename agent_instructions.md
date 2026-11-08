# Instructions

## Analysis:

Attached is a list of information about processes that did not exit with error code `0`
The format of these will be as follows:

```
[
  {
    "id": 1,
    "process_name": "broken_worker",
    "comm": "broken_worker",
    "error_code": 127,
    "exit_code": 127
  }
]
```

Determine which of them could be due to the system not functioning correctly  
Return a list of IDs for which you'd like more information about. You would like more information about any process you think might be erroring  
Additional information will be returned in the format of:

```
{
  "id": 1,
  "timestamp": 1794093000,
  "boot_id": "b50627c5-373f-49cf-82d5-a43ae178309f",
  "pid": 9001,
  "tgid": 9001,
  "parent_pid": 1,
  "process_name": "broken_worker",
  "comm": "broken_worker",
  "cmdline": "broken_worker --retry",
  "error_code": 127,
  "exit_code": 127,
  "exit_signal": null,
  "summary": "Process 'broken_worker' (PID 9001) exited with code 127",
  "notification_body": "Command: broken_worker --retry\nPID: 9001\nExit Code: 127",
  "stdout": "starting worker...",
  "stderr": "command not found"
}
```

## Fixing:

For each process that is functioning incorrectly, analyze why it is and determine a plan on how to fix it  
You may only request the most recent ID for a given process name, for a given error code  
This is to reduce the number of required calls  
Do not create a plan to fix until you have further investigated the error, if it is even necessary to fix  
Upon creating the plan, return to the user a description of the error, and a list of separate commands on how to fix it

## Formatting:

Everything in angle brackets should be based on your analysis and input  
and everything in parenthesis should not be printed, but is for additional instructions:  
Your output should be of the following format:

Initial return:

```
process_list: <p_list>
(This should only be for the processes that you wish to get more information above)
(Reference the Fixing section for more information on what that should look like)
(It should be formatted like a js array of strings)
```

After getting more information on a given call:

```
# <Process Name>:
<Error Description, plan to fix>

(You should not fully determine any commands after the first until seeing the output of the first)
## <first_command>
```

For each further command:

```
## <additional_command>
```

If at any point, you are told `COMMAND NOT APPROVED`, cease all further commands **For that pid**  
You may then switch to a new PID to attempt to analyze and potentially fix  
Upon finishing the attempted fix for a given PID, return `FIX COMPLETE`  