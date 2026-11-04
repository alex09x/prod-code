#!/usr/bin/env python3
# prod-code — Remote code intelligence
# Copyright (c) 2026 Alexander Panasenko
# Contact: alex@prod.codes
# Author: https://prod.codes/about/
# Project: https://github.com/alex09x/prod-code
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Read and validate LLVM coverage reports."""

import json


def validate_report(report: dict) -> None:
    """Require measurements, including explicit zeroes; absence is not zero coverage."""
    if not isinstance(report, dict):
        raise ValueError("coverage report must be a JSON object")
    data = report.get("data")
    if not isinstance(data, list):
        raise ValueError("coverage report missing 'data' list")
    if not data:
        raise ValueError("coverage report 'data' is empty")
    count = 0
    for item in data:
        if not isinstance(item, dict) or not isinstance(item.get("files"), list):
            raise ValueError("each coverage data record must contain a files list")
        for entry in item["files"]:
            if not isinstance(entry, dict):
                raise ValueError("each coverage file entry must be an object")
            filename = entry.get("filename")
            if not isinstance(filename, str) or not filename.strip():
                raise ValueError("coverage file entry has no valid filename")
            summary = entry.get("summary")
            regions = summary.get("regions") if isinstance(summary, dict) else None
            if not isinstance(regions, dict):
                raise ValueError(f"missing region measurements for {filename}")
            total, covered = regions.get("count"), regions.get("covered")
            if (type(total) is not int or type(covered) is not int
                    or total < 0 or covered < 0 or covered > total):
                raise ValueError(f"invalid region measurements for {filename}")
            count += 1
    if not count:
        raise ValueError("coverage report contains no valid file entries")


def collect(report_path: str | None, root: str) -> dict:
    if report_path:
        try:
            with open(report_path, "r", encoding="utf-8") as handle:
                report = json.load(handle)
        except OSError as err:
            raise ValueError(f"cannot read coverage report {report_path}: {err}") from err
        except json.JSONDecodeError as err:
            raise ValueError(f"invalid JSON in coverage report {report_path}: {err}") from err
        validate_report(report)
        return report
    raise ValueError(
        "Generating a coverage report requires Mitts support for llvm-cov "
        "--output-path (alex09x/mitts#31); supply an existing report with --report"
    )
