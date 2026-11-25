$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$versionSections = @('Windows','Firmware','Display adapters','Storage','Installed desktop software')
$versionData = [ordered]@{}

function Show-Section($Name, [scriptblock]$Read) {
    if ($ParsleyVersionSnapshot) {
        if ($Name -in $versionSections) {
            try {
                $versionData[$Name] = @(& $Read)
            } catch {
                $versionData[$Name] = $null
            }
        }
        return
    }
    Write-Output $Name
    try {
        $result = & $Read
        if ($result) {
            Write-Output ($result | Format-Table -AutoSize | Out-String -Width 180).Trim()
        } else {
            Write-Output 'No entries reported.'
        }
    } catch {
        Write-Output 'Windows did not return this inventory category.'
    }
    Write-Output ''
}

function Read-HardwareClass($Class) {
    Get-CimInstance -ClassName $Class -OperationTimeoutSec 5
}

Show-Section 'Windows' {
    Read-HardwareClass Win32_OperatingSystem | Select-Object Caption,Version,BuildNumber,OSArchitecture
}
Show-Section 'Processor' {
    Read-HardwareClass Win32_Processor | Select-Object Name,NumberOfCores,NumberOfLogicalProcessors
}
Show-Section 'Firmware' {
    Read-HardwareClass Win32_BIOS | Select-Object Manufacturer,SMBIOSBIOSVersion
}
Show-Section 'Mainboard' {
    Read-HardwareClass Win32_BaseBoard | Select-Object Manufacturer,Product
}
Show-Section 'Memory modules' {
    Read-HardwareClass Win32_PhysicalMemory | Select-Object Manufacturer,ConfiguredClockSpeed,
        @{n='Capacity (GiB)';e={[math]::Round($_.Capacity / 1GB, 2)}}
}
Show-Section 'Display adapters' {
    Read-HardwareClass Win32_VideoController | Select-Object Name,DriverVersion
}
Show-Section 'Monitors' {
    Read-HardwareClass Win32_DesktopMonitor | Select-Object Name,ScreenWidth,ScreenHeight,Status
}
Show-Section 'Batteries' {
    Read-HardwareClass Win32_Battery | Select-Object Name,EstimatedChargeRemaining,Status
}
Show-Section 'Storage' {
    Read-HardwareClass Win32_DiskDrive | Select-Object Model,FirmwareRevision,
        @{n='Capacity (GiB)';e={[math]::Round($_.Size / 1GB, 1)}}
}
Show-Section 'Storage reliability counters' {
    Get-PhysicalDisk -ErrorAction Stop | ForEach-Object {
        $disk = $_
        $metrics = $null
        try { $metrics = Get-StorageReliabilityCounter -PhysicalDisk $disk -ErrorAction Stop } catch {}
        [pscustomobject]@{
            Drive = $disk.FriendlyName
            Status = $disk.HealthStatus
            TemperatureC = $metrics.Temperature
            WearPercent = $metrics.Wear
            PowerOnHours = $metrics.PowerOnHours
            UncorrectedReads = $metrics.ReadErrorsUncorrected
            UncorrectedWrites = $metrics.WriteErrorsUncorrected
        }
    }
}
Show-Section 'Network adapters' {
    Read-HardwareClass Win32_NetworkAdapter | Where-Object PhysicalAdapter | Select-Object Name,NetEnabled
}
Show-Section 'Devices' {
    Read-HardwareClass Win32_PnPEntity | Where-Object Name | Sort-Object PNPClass,Name |
        Select-Object PNPClass,Name,Status
}
Show-Section 'Installed desktop software' {
    $paths = @(
        'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*'
        'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*'
        'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*'
    )
    $registryErrors = @()
    $products = @(Get-ItemProperty -Path $paths -ErrorAction SilentlyContinue -ErrorVariable registryErrors)
    if ($ParsleyVersionSnapshot -and ($registryErrors | Where-Object { $_.CategoryInfo.Category -ne 'ObjectNotFound' })) {
        throw 'Could not read all software version locations.'
    }
    $products |
        Where-Object { $_.DisplayName -and $_.SystemComponent -ne 1 } |
        Select-Object DisplayName,DisplayVersion,Publisher |
        Sort-Object DisplayName,DisplayVersion,Publisher -Unique
}
Show-Section 'Windows services' {
    Get-Service | Sort-Object DisplayName | Select-Object DisplayName,Status,StartType
}

if ($ParsleyVersionSnapshot) {
    ConvertTo-Json -InputObject $versionData -Depth 6 -Compress
}
