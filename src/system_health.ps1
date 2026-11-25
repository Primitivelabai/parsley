$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)

function Show-Section($Name, [scriptblock]$Read) {
    Write-Output $Name
    try {
        $result = & $Read
        if ($result) {
            Write-Output ($result | Format-Table -AutoSize | Out-String -Width 180).Trim()
        } else {
            Write-Output 'No counters available.'
        }
    } catch {
        Write-Output 'Windows did not return these counters.'
    }
    Write-Output ''
}

function Read-CounterClass($Class, $Filter) {
    if ($Filter) {
        Get-CimInstance -ClassName $Class -Filter $Filter -OperationTimeoutSec 5
    } else {
        Get-CimInstance -ClassName $Class -OperationTimeoutSec 5
    }
}

$logicalProcessors = [Environment]::ProcessorCount
Show-Section 'CPU' {
    Read-CounterClass Win32_PerfFormattedData_PerfOS_Processor "Name='_Total'" |
        Select-Object @{n='Busy (%)';e={$_.PercentProcessorTime}}
}
Show-Section 'Physical memory' {
    Read-CounterClass Win32_OperatingSystem |
        Select-Object @{n='In use (GiB)';e={[math]::Round(($_.TotalVisibleMemorySize - $_.FreePhysicalMemory) / 1MB, 2)}},
                      @{n='Total (GiB)';e={[math]::Round($_.TotalVisibleMemorySize / 1MB, 2)}}
}
Show-Section 'Physical disk throughput' {
    Read-CounterClass Win32_PerfFormattedData_PerfDisk_PhysicalDisk |
        Where-Object Name -ne '_Total' |
        Select-Object Name,@{n='Read (MiB/s)';e={[math]::Round($_.DiskReadBytesPersec / 1MB, 2)}},
                           @{n='Write (MiB/s)';e={[math]::Round($_.DiskWriteBytesPersec / 1MB, 2)}}
}
Show-Section 'Network interface throughput' {
    Read-CounterClass Win32_PerfFormattedData_Tcpip_NetworkInterface |
        Select-Object Name,@{n='Received (KiB/s)';e={[math]::Round($_.BytesReceivedPersec / 1KB, 1)}},
                           @{n='Sent (KiB/s)';e={[math]::Round($_.BytesSentPersec / 1KB, 1)}}
}
Show-Section 'Active GPU engine counters' {
    Read-CounterClass Win32_PerfFormattedData_GPUPerformanceCounters_GPUEngine |
        Where-Object UtilizationPercentage -gt 0 |
        Sort-Object UtilizationPercentage -Descending |
        Select-Object -First 20 Name,@{n='Busy (%)';e={$_.UtilizationPercentage}}
}
Show-Section 'Top processes by CPU use' {
    Read-CounterClass Win32_PerfFormattedData_PerfProc_Process |
        Where-Object { $_.IDProcess -ne 0 } |
        Sort-Object PercentProcessorTime -Descending |
        Select-Object -First 25 Name,IDProcess,
            @{n='Total CPU (%)';e={[math]::Round($_.PercentProcessorTime / $logicalProcessors, 1)}},
            @{n='Working set (MiB)';e={[math]::Round($_.WorkingSet / 1MB, 1)}}
}
