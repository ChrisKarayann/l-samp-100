import { TestBed } from '@angular/core/testing';
import { App } from './app';
import { provideZonelessChangeDetection } from '@angular/core';

describe('App', () => {
  beforeEach(async () => {
    await TestBed.configureTestingModule({
      imports: [App],
      providers: [provideZonelessChangeDetection()],
    }).compileComponents();
  });

  it('should create the app', () => {
    const fixture = TestBed.createComponent(App);
    const app = fixture.componentInstance;
    expect(app).toBeTruthy();
  });

  it('should initialize with signals', () => {
    const fixture = TestBed.createComponent(App);
    const app = fixture.componentInstance;
    fixture.detectChanges();
    
    expect(app.pads().length).toBeGreaterThan(0);
    expect(app.matrix()).toBe('2x4');
    expect(app.accentColor()).toBe('#00ffcc');
  });
});

