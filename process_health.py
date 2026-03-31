#!/usr/bin/env python3
"""
Process Apple Health Data
This script analyzes your Apple Health export focusing on:
- Heart health (HR, HRV, VO2 max)
- Respiratory (SpO2, respiratory rate)
- Exercise & activity
- Sleep
- Weight

Usage: python3 process_health.py
"""
import xml.etree.ElementTree as ET
import pandas as pd
from datetime import datetime, timedelta
from zoneinfo import ZoneInfo
from collections import defaultdict
import os
import sys

# Paths
BASE_DIR = "/Users/sloveless/Documents/Health"
XML_PATH = os.path.join(BASE_DIR, "apple_health_export/export.xml")
OUTPUT_DIR = os.path.join(BASE_DIR, "analysis_output")

print("="*80)
print("APPLE HEALTH DATA ANALYZER")
print("="*80)

if not os.path.exists(XML_PATH):
    print(f"\n❌ Error: Cannot find {XML_PATH}")
    print("\nPlease make sure you've exported and unzipped your Apple Health data to:")
    print(f"  {BASE_DIR}/apple_health_export/")
    sys.exit(1)

os.makedirs(OUTPUT_DIR, exist_ok=True)

print(f"\nInput file: {XML_PATH}")
print(f"File size: {os.path.getsize(XML_PATH) / (1024**3):.2f} GB")
print(f"Output directory: {OUTPUT_DIR}")

# Define metrics of interest
METRICS = {
    'heart_rate': 'HKQuantityTypeIdentifierHeartRate',
    'resting_heart_rate': 'HKQuantityTypeIdentifierRestingHeartRate',
    'walking_heart_rate': 'HKQuantityTypeIdentifierWalkingHeartRateAverage',
    'hrv': 'HKQuantityTypeIdentifierHeartRateVariabilitySDNN',
    'vo2_max': 'HKQuantityTypeIdentifierVO2Max',
    'blood_oxygen': 'HKQuantityTypeIdentifierOxygenSaturation',
    'respiratory_rate': 'HKQuantityTypeIdentifierRespiratoryRate',
    'sleep_analysis': 'HKCategoryTypeIdentifierSleepAnalysis',
    'weight': 'HKQuantityTypeIdentifierBodyMass',
    'body_fat': 'HKQuantityTypeIdentifierBodyFatPercentage',
    'active_energy': 'HKQuantityTypeIdentifierActiveEnergyBurned',
    'exercise_time': 'HKQuantityTypeIdentifierAppleExerciseTime',
    'steps': 'HKQuantityTypeIdentifierStepCount',
    'flights_climbed': 'HKQuantityTypeIdentifierFlightsClimbed',
}

# Key dates - make timezone-aware to match health data
pacific = ZoneInfo("America/Los_Angeles")
ILLNESS_START = datetime(2025, 8, 22, tzinfo=pacific)
CURRENT = datetime.now(pacific)
YEAR_AGO = CURRENT - timedelta(days=365)
SIX_MONTHS_AGO = CURRENT - timedelta(days=180)

print("\n" + "="*80)
print("PARSING XML FILE (This will take several minutes...)")
print("="*80)

data = defaultdict(list)
workouts = []
record_count = 0
parse_start = datetime.now()

try:
    # Use iterparse for memory efficiency
    context = ET.iterparse(XML_PATH, events=('end',))
    
    for event, elem in context:
        if elem.tag == 'Record':
            record_type = elem.get('type')
            
            # Check if this is a metric we're tracking
            for metric_name, health_type in METRICS.items():
                if record_type == health_type:
                    try:
                        start_date_str = elem.get('startDate')
                        if start_date_str:
                            # Handle timezone - keep as timezone-aware
                            start_date = datetime.fromisoformat(start_date_str.replace(' +0000', ''))
                            value = elem.get('value')
                            
                            if value:
                                data[metric_name].append({
                                    'date': start_date,
                                    'value': float(value),
                                    'source': elem.get('sourceName', '')
                                })
                    except (ValueError, TypeError) as e:
                        pass  # Skip malformed records
                    break
            
            record_count += 1
            if record_count % 500000 == 0:
                elapsed = (datetime.now() - parse_start).total_seconds()
                rate = record_count / elapsed if elapsed > 0 else 0
                print(f"  Processed: {record_count:,} records ({rate:.0f}/sec)")
        
        elif elem.tag == 'Workout':
            try:
                start_date_str = elem.get('startDate')
                if start_date_str:
                    workouts.append({
                        'date': datetime.fromisoformat(start_date_str.replace(' +0000', '')),
                        'type': elem.get('workoutActivityType', 'Unknown'),
                        'duration': float(elem.get('duration', 0)),
                        'distance': float(elem.get('totalDistance')) if elem.get('totalDistance') else None,
                        'energy': float(elem.get('totalEnergyBurned')) if elem.get('totalEnergyBurned') else None
                    })
            except (ValueError, TypeError):
                pass
        
        # Clear to save memory
        elem.clear()
    
    parse_time = (datetime.now() - parse_start).total_seconds()
    print(f"\n✓ Parsing complete! ({parse_time:.1f} seconds)")
    print(f"✓ Total records processed: {record_count:,}")
    print(f"✓ Total workouts found: {len(workouts):,}")
    
    # Create detailed report
    report_lines = []
    report_lines.append("="*80)
    report_lines.append("APPLE HEALTH DATA ANALYSIS")
    report_lines.append("="*80)
    report_lines.append(f"\nGenerated: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
    report_lines.append(f"Data source: {XML_PATH}")
    report_lines.append(f"Total records analyzed: {record_count:,}")
    report_lines.append(f"Analysis period: Focus on past year with illness comparison (Aug 22, 2025)")
    
    # Save and analyze each metric
    print("\n" + "="*80)
    print("GENERATING ANALYSIS")
    print("="*80)
    
    for metric_name, records in sorted(data.items()):
        if not records:
            continue
        
        df = pd.DataFrame(records).sort_values('date')
        
        # Save CSV
        csv_path = os.path.join(OUTPUT_DIR, f"{metric_name}.csv")
        df.to_csv(csv_path, index=False)
        print(f"  ✓ Saved: {metric_name}.csv ({len(df):,} records)")
        
        # Analysis for report
        recent_year = df[df['date'] >= YEAR_AGO]
        
        if len(recent_year) > 0:
            report_lines.append(f"\n{'='*80}")
            report_lines.append(metric_name.upper().replace('_', ' '))
            report_lines.append(f"{'='*80}")
            report_lines.append(f"Total records: {len(df):,}")
            report_lines.append(f"Date range: {df['date'].min().strftime('%Y-%m-%d')} to {df['date'].max().strftime('%Y-%m-%d')}")
            report_lines.append(f"\nPast Year Statistics:")
            report_lines.append(f"  Mean: {recent_year['value'].mean():.2f}")
            report_lines.append(f"  Median: {recent_year['value'].median():.2f}")
            report_lines.append(f"  Min: {recent_year['value'].min():.2f}")
            report_lines.append(f"  Max: {recent_year['value'].max():.2f}")
            report_lines.append(f"  Std Dev: {recent_year['value'].std():.2f}")
            
            # Pre vs Post illness comparison
            pre_illness = df[df['date'] < ILLNESS_START]
            post_illness = df[df['date'] >= ILLNESS_START]
            
            if len(pre_illness) > 0 and len(post_illness) > 0:
                # Use 90 days before illness for comparison
                pre_recent = pre_illness[pre_illness['date'] >= ILLNESS_START - timedelta(days=90)]
                
                if len(pre_recent) > 0:
                    report_lines.append(f"\nIllness Impact Analysis (Aug 22, 2025):")
                    report_lines.append(f"  Pre-illness (90d avg): {pre_recent['value'].mean():.2f}")
                    report_lines.append(f"  Post-illness avg: {post_illness['value'].mean():.2f}")
                    change = post_illness['value'].mean() - pre_recent['value'].mean()
                    pct_change = (change / pre_recent['value'].mean() * 100) if pre_recent['value'].mean() != 0 else 0
                    report_lines.append(f"  Change: {change:+.2f} ({pct_change:+.1f}%)")
            
            # Special analysis for certain metrics
            if metric_name == 'blood_oxygen':
                low_readings = recent_year[recent_year['value'] < 94]
                if len(low_readings) > 0:
                    report_lines.append(f"\n⚠️  Low SpO2 readings (<94%): {len(low_readings)} occurrences")
                    report_lines.append(f"    Lowest: {low_readings['value'].min():.1f}%")
            
            if metric_name == 'resting_heart_rate':
                report_lines.append(f"\nMonthly trends:")
                recent_year_copy = recent_year.copy()
                recent_year_copy['month'] = recent_year_copy['date'].dt.to_period('M')
                monthly = recent_year_copy.groupby('month')['value'].mean()
                for month, value in monthly.tail(6).items():
                    report_lines.append(f"  {month}: {value:.1f} bpm")
    
    # Workout analysis
    if workouts:
        df_workouts = pd.DataFrame(workouts).sort_values('date')
        csv_path = os.path.join(OUTPUT_DIR, "workouts.csv")
        df_workouts.to_csv(csv_path, index=False)
        print(f"  ✓ Saved: workouts.csv ({len(df_workouts):,} records)")
        
        recent_workouts = df_workouts[df_workouts['date'] >= YEAR_AGO]
        
        report_lines.append(f"\n{'='*80}")
        report_lines.append("WORKOUTS")
        report_lines.append(f"{'='*80}")
        report_lines.append(f"Total workouts (past year): {len(recent_workouts):,}")
        
        workout_types = recent_workouts['type'].value_counts()
        report_lines.append(f"\nTop workout types:")
        for wtype, count in workout_types.head(10).items():
            avg_duration = recent_workouts[recent_workouts['type'] == wtype]['duration'].mean()
            report_lines.append(f"  {wtype}: {count} workouts (avg {avg_duration/60:.1f} min)")
        
        # Pre vs post illness workout frequency
        pre_workouts = df_workouts[df_workouts['date'] < ILLNESS_START]
        post_workouts = df_workouts[df_workouts['date'] >= ILLNESS_START]
        pre_recent = pre_workouts[pre_workouts['date'] >= ILLNESS_START - timedelta(days=90)]
        
        if len(pre_recent) > 0 and len(post_workouts) > 0:
            report_lines.append(f"\nWorkout Frequency:")
            report_lines.append(f"  Pre-illness (90d): {len(pre_recent)} ({len(pre_recent)/90:.2f}/day)")
            days_since = (CURRENT - ILLNESS_START).days
            report_lines.append(f"  Post-illness: {len(post_workouts)} ({len(post_workouts)/days_since:.2f}/day)")
    
    # Save report
    report_text = "\n".join(report_lines)
    report_path = os.path.join(OUTPUT_DIR, "health_analysis_report.txt")
    with open(report_path, 'w') as f:
        f.write(report_text)
    
    print(f"\n  ✓ Saved: health_analysis_report.txt")
    
    # Print summary to console
    print("\n" + "="*80)
    print("ANALYSIS COMPLETE")
    print("="*80)
    print(f"\nOutput directory: {OUTPUT_DIR}")
    print(f"\nGenerated files:")
    print(f"  - health_analysis_report.txt (comprehensive report)")
    for metric_name in sorted(data.keys()):
        if data[metric_name]:
            print(f"  - {metric_name}.csv")
    if workouts:
        print(f"  - workouts.csv")
    
    print(f"\n{'='*80}")
    print("KEY FINDINGS SUMMARY")
    print(f"{'='*80}")
    
    # Print just the key metrics
    for metric in ['resting_heart_rate', 'hrv', 'blood_oxygen', 'weight']:
        if metric in data and data[metric]:
            df = pd.DataFrame(data[metric])
            df = df[df['date'] >= YEAR_AGO]
            if len(df) > 0:
                print(f"\n{metric.replace('_', ' ').title()}")
                print(f"  Current year average: {df['value'].mean():.1f}")
                
                # Show trend
                recent_30d = df[df['date'] >= CURRENT - timedelta(days=30)]
                if len(recent_30d) > 0:
                    print(f"  Last 30 days average: {recent_30d['value'].mean():.1f}")
    
    print(f"\n{'='*80}")
    print("✓ Analysis complete! Review the full report for detailed findings.")
    print(f"{'='*80}\n")
    
except KeyboardInterrupt:
    print("\n\n⚠️  Analysis interrupted by user")
    sys.exit(1)
except Exception as e:
    print(f"\n\n❌ Error during analysis: {e}")
    import traceback
    traceback.print_exc()
    sys.exit(1)
