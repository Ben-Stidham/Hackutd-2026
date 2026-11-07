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
Upon creating the plan, return to the user a description of the error, and a list of separate commands on how to fix it
